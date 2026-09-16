//! Trusted local launcher; browser requests never choose paths/binaries.
mod handoff;
use floe_app_core::{
    cache,
    jobdeck::index::parse_levels,
    managed::{Limits, Resources},
    native::{Discovery, Indexer},
    registered::{AccessScope, RegisteredSource},
    render::RenderOptions,
    shots,
    view::ControllerOptions,
    Error, Result,
};
use floe_web::{
    service::Service,
    transport::{self, Gateway, BUNDLE},
};
use serde_json::{json, Value};
use std::{
    cmp::Ordering as NumberSign,
    collections::BTreeSet,
    fs::{self, DirBuilder, OpenOptions},
    io::{Read, Write},
    net::{Ipv4Addr, TcpListener},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const HELP: &str = "Usage: floe2-web view [SOURCE ...] [OPTIONS]
       floe2-web [OPTIONS] SOURCE ...     (view shorthand)

  --multi                  Independent workspace; do not own/forward the default instance
  --goto X,Y[,WIDTH]        Initial centre; omitted width keeps fit zoom (um)
  --depth full|N            Default 0; goto/DRC/jobdeck default full (999 = full)
  --detail low|medium|high|exact  Initial detail (default medium)
  --thin auto|keep|cull     Thin-page policy (default auto)
  --mode level|chip|layer  Jobdeck view mode (default level)
  --level N,N,...          Initial jobdeck levels (otherwise ask before open)
  --frames [on|off]        Initial hierarchy frames (default on; bare = on)
  --labels [on|off]        Initial labels (default on; frames off suppresses them)
  --no-labels / --no-frames  Aliases for off
  --label-font-px N        Label size (6..96 device px, default 14)
  --mono                   Initial monochrome display
  --jobs N                 Decode workers (environment/default up to 8)
  --raster-jobs N          Raster workers (environment/default up to 4)
  --budget-mb N            Decoded page budget (default 1024)
  --png / --raw            Frame transfer (default raw)
  --frame-cache on|off      Retained frame reuse + layout margin (default on)
  --refinement on|off      On follows round env; off forces direct-final (default effectively off)
  --stream-kb N            Legacy compatibility: 0 forces off; positive follows round env (not KB)
  --render-debug           Numeric worker-frame diagnostics to stderr; independent workspace
  --dump                   Keep recent frame/display pixels in browser memory; explicit downloads
  --perf-baseline           Frames/labels/refinement/frame reuse off; caches stay
  --root DIRECTORY         Additional approved dependency/file-picker root, repeatable
  --drc RESULTS.db|PACK.ice Register DRC on first source; pack build needs owner approval
  --drc-waives FILE        Explicit existing waive sidecar (requires --drc)
  --drc-rules FILE         Explicit existing SVRF rules.json (requires --drc)
  --floe-reviewer TAG      Read derived adjacent/legacy temporary notes and waives; no writes
                          ASCII uses a current adjacent ICE, otherwise ASCII without sidecars
  --drc-reviewer TAG       Enable owner note publication for this fixed tag (requires --drc)
  --drc-edit-waives        Also enable approved waive writes for --drc-reviewer
  --port N                 Loopback port (default random)
  --no-open                Do not launch a browser; use the private session file
  --firefox PATH           Explicit Firefox binary (or FLOE_FIREFOX_BIN)
  --session-file FILE      New 0600 session JSON; never overwrite
  --help                   Show this help

This development command does not replace the GTK floe2 launcher.
Default instance: same UID + DISPLAY (headless supported), separate from GTK.
Later calls queue an open in that window; acceptance is not a rendered-frame ACK.
Explicit process/DRC/session options start independently. --no-open alone may own the instance.
No automatic indexing or Python fallback. Paths are local-launcher inputs only.
The file picker lists source parents and --root directories (launch directory
when both are absent), never arbitrary server paths. Hidden/cache files and
symlinks are not listed. Roots stay fixed for the lifetime of this workspace.
Binds only 127.0.0.1; stops on Ctrl+C or End session.
Known commands take precedence over bare filenames. Use ./index or -- index
for a source named index; put options before -- for leading-dash filenames.
Managed capacity: 16 CPU slots, 4 reserved for foreground; index jobs <=12.
Decode+raster plus file catalogue (1 slot + 192 MiB) must fit 16 slots
(DRC reserves 1 extra slot + 256 MiB;
SVRF metadata reserves another 256 MiB, with no extra CPU worker).
DRC reads the explicit file unless --floe-reviewer selects its current adjacent ICE.
No implicit indexing or ambient reviewer selection; read-only selection grants no writes.
Default round size is direct-final. --refinement on (like omission) follows
FLOE_RUST_ROUND_PAGES; it does not invent a progressive/byte/time policy.
--refinement off, --stream-kb 0 and --perf-baseline override that setting.
Positive --stream-kb is not a byte budget; its magnitude was unused by Rust.
It conflicts with refinement off/perf-baseline. Repeated stream values use
the last integer. Explicit --stream-kb always starts an independent workspace.
Deck margin is unsupported.
--stream-target-ms, --lod, --hairline and --thin-um
are not migrated; they are rejected, never silently ignored.
--dump starts an independent workspace. About has the capture toggle/downloads.
It copies displayed pixels, affects timing/memory and never writes server /tmp PNGs.
--hairline/--thin-um affected the legacy KLayout planner, not Rust renderd.
Use --thin auto|keep|cull for the existing Rust thin-page policy, not as an
equivalent frame-lattice control. --render-debug excludes paths, coordinates,
source text and raw worker lines; synchronous stderr can affect timing.
FLOE_JOBDECK_LEVELS=all|ask|N,N... supplies the default level choice (--level wins).
--perf-baseline leaves decoded caches and geometry cut unchanged; no live LOD toggle.
It may start an independent empty window; its display settings apply to the first file choice.
FLOE_FILL_EDIT (nonempty) enables session bitmap-slot editing and, separately,
shared design-default publication with its own preview and explicit approval.
The session link is a one-time credential; do not share or log it.";

#[derive(Debug)]
pub struct Command {
    help: bool,
    independent: bool,
    sources: Vec<PathBuf>,
    roots: Vec<PathBuf>,
    initial: Value,
    mode: String,
    levels: Option<BTreeSet<i64>>,
    jobs: Option<u16>,
    raster: Option<u16>,
    budget: Option<u64>,
    raw: Option<bool>,
    frame_cache: bool,
    direct_final: bool,
    render_debug: bool,
    dump: bool,
    perf_baseline: bool,
    port: u16,
    no_open: bool,
    session_file: Option<PathBuf>,
    firefox: Option<PathBuf>,
    drc: Option<PathBuf>,
    drc_waives: Option<PathBuf>,
    drc_rules: Option<PathBuf>,
    drc_reviewer: Option<String>,
    read_reviewer: Option<String>,
    drc_edit_waives: bool,
}
pub fn parse(args: &[String]) -> Result<Command> {
    let mut c = Command {
        help: false,
        independent: false,
        sources: Vec::new(),
        roots: Vec::new(),
        initial: json!({}),
        mode: "level".into(),
        levels: None,
        jobs: None,
        raster: None,
        budget: None,
        raw: None,
        frame_cache: true,
        direct_final: false,
        render_debug: false,
        dump: false,
        perf_baseline: false,
        port: 0,
        no_open: false,
        session_file: None,
        firefox: None,
        drc: None,
        drc_waives: None,
        drc_rules: None,
        drc_reviewer: None,
        read_reviewer: None,
        drc_edit_waives: false,
    };
    let mut i = 1;
    let mut positional = false;
    let mut refinement_off = false;
    let mut stream_sign = None;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if !positional && arg == "--" {
            positional = true;
            continue;
        }
        if positional || !arg.starts_with('-') {
            c.sources.push(PathBuf::from(arg));
            continue;
        }
        let (key, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(k, v)| (k, Some(v)));
        let toggle_value = inline.is_some() || args.get(i).is_some_and(|v| v == "on" || v == "off");
        if matches!(
            key,
            "--multi"
                | "--jobs"
                | "--raster-jobs"
                | "--budget-mb"
                | "--png"
                | "--raw"
                | "--frame-cache"
                | "--stream-kb"
                | "--render-debug"
                | "--dump"
                | "--perf-baseline"
                | "--port"
                | "--session-file"
                | "--firefox"
                | "--drc"
                | "--drc-waives"
                | "--drc-rules"
                | "--drc-reviewer"
                | "--floe-reviewer"
                | "--drc-edit-waives"
        ) {
            c.independent = true;
        }
        let mut value = || -> Result<&str> {
            if let Some(v) = inline {
                return Ok(v);
            }
            let v = args
                .get(i)
                .filter(|v| !v.starts_with("--"))
                .ok_or_else(|| Error::input(format!("{key} requires a value")))?;
            i += 1;
            Ok(v)
        };
        let flag = || -> Result<()> {
            if inline.is_some() {
                Err(Error::input(format!("{key} takes no value")))
            } else {
                Ok(())
            }
        };
        match key {
            "--multi" => flag()?,
            "--help" | "-h" => {
                flag()?;
                c.help = true;
                return Ok(c);
            }
            "--no-open" => {
                flag()?;
                c.no_open = true;
            }
            "--no-labels" => {
                flag()?;
                c.initial["labels"] = json!(false);
            }
            "--labels" | "--frames" => {
                let on = if toggle_value {
                    match value()? {
                        "on" => true,
                        "off" => false,
                        _ => return Err(Error::input(format!("{key} must be on or off"))),
                    }
                } else {
                    true
                };
                c.initial[&key[2..]] = json!(on);
            }
            "--no-frames" => {
                flag()?;
                c.initial["frames"] = json!(false);
            }
            "--label-font-px" => {
                c.initial["font_px"] = json!(number(value()?, 6, 96, key)?);
            }
            "--mono" => {
                flag()?;
                c.initial["mono"] = json!(true);
            }
            "--png" => {
                flag()?;
                c.raw = Some(false);
            }
            "--raw" => {
                flag()?;
                c.raw = Some(true);
            }
            "--goto" => {
                let text = value()?;
                let parts: Vec<_> = if text.contains([',', ';']) {
                    text.split([',', ';'])
                        .filter(|s| !s.trim().is_empty())
                        .collect()
                } else {
                    text.split_whitespace().collect()
                };
                if !(2..=3).contains(&parts.len()) {
                    return Err(Error::input("goto requires X,Y[,WIDTH]"));
                }
                let values = parts
                    .into_iter()
                    .map(shots::length)
                    .collect::<Result<Vec<_>>>()?;
                if values.get(2).is_some_and(|w| *w <= 0.) {
                    return Err(Error::input("goto width must be positive"));
                }
                c.initial["navigation"] = json!({"kind":"goto","center_um":[values[0].to_string(),values[1].to_string()]});
                if let Some(w) = values.get(2) {
                    c.initial["navigation"]["width_um"] = json!(w.to_string());
                }
            }
            "--refinement" => {
                refinement_off = match value()? {
                    "on" => false,
                    "off" => true,
                    _ => return Err(Error::input("refinement must be on or off")),
                };
            }
            "--stream-kb" => {
                stream_sign = Some(stream_kb_sign(value()?)?);
            }
            "--render-debug" => {
                flag()?;
                c.render_debug = true;
            }
            "--stream-target-ms" => return Err(Error::input(
                "--stream-target-ms controlled legacy adaptive streaming and was unused by Rust renderd; use --refinement off for direct-final rendering",
            )),
            "--hairline" | "--thin-um" => return Err(Error::input(
                "--hairline/--thin-um were legacy KLayout planner controls, not Rust renderd controls; --thin keep|cull is a separate thin-page policy, not an equivalent frame control",
            )),
            "--lod" => return Err(Error::input(
                "view --lod was not sent to Rust renderd; a live LOD policy switch is not migrated (index --lod controls generation, not display)",
            )),
            "--dump" => { flag()?; c.dump = true; }
            "--floe-reviewer" => c.read_reviewer = Some(value()?.to_owned()),
            "--perf-baseline" => {
                flag()?;
                c.perf_baseline = true;
            }
            "--frame-cache" => {
                c.frame_cache = match value()? {
                    "on" => true,
                    "off" => false,
                    _ => return Err(Error::input("frame-cache must be on or off")),
                };
            }
            "--depth" => {
                c.initial["depth"] = json!(startup_depth(value()?)?);
            }
            "--detail" => {
                let v = value()?;
                if !["low", "medium", "high", "exact"].contains(&v) {
                    return Err(Error::input("invalid detail"));
                }
                c.initial["detail"] = json!(v);
            }
            "--thin" => {
                let v = value()?;
                if !["auto", "keep", "cull"].contains(&v) {
                    return Err(Error::input("invalid thin policy"));
                }
                c.initial["thin"] = json!(v);
            }
            "--mode" => {
                let v = value()?;
                if !["level", "chip", "layer"].contains(&v) {
                    return Err(Error::input("mode must be level, chip or layer"));
                }
                c.mode = v.into();
            }
            "--level" => c.levels = Some(parse_levels(value()?)?),
            "--jobs" => c.jobs = Some(number(value()?, 1, 16, key)? as u16),
            "--raster-jobs" => c.raster = Some(number(value()?, 1, 16, key)? as u16),
            "--budget-mb" => c.budget = Some(number(value()?, 1, 2048, key)?),
            "--port" => c.port = number(value()?, 0, 65535, key)? as u16,
            "--root" => c.roots.push(PathBuf::from(value()?)),
            "--session-file" => c.session_file = Some(PathBuf::from(value()?)),
            "--firefox" => c.firefox = Some(PathBuf::from(value()?)),
            "--drc" => c.drc = Some(PathBuf::from(value()?)),
            "--drc-waives" => c.drc_waives = Some(PathBuf::from(value()?)),
            "--drc-rules" => c.drc_rules = Some(PathBuf::from(value()?)),
            "--drc-reviewer" => c.drc_reviewer = Some(value()?.to_owned()),
            "--drc-edit-waives" => {
                flag()?;
                c.drc_edit_waives = true;
            }
            _ => return Err(Error::input(format!("unsupported view option: {key}"))),
        }
    }
    if (c.drc_waives.is_some()
        || c.drc_rules.is_some()
        || c.drc_reviewer.is_some()
        || c.read_reviewer.is_some())
        && c.drc.is_none()
    {
        return Err(Error::input(
            "--drc-waives / --drc-rules / --drc-reviewer / --floe-reviewer require --drc",
        ));
    }
    if let Some(tag) = &c.drc_reviewer {
        floe_app_core::drc::waive_paths(c.drc.as_ref().unwrap(), tag)?;
    }
    if let Some(tag) = &c.read_reviewer {
        floe_app_core::drc::waive_paths(c.drc.as_ref().unwrap(), tag)?;
        if c.drc_reviewer.is_some() || c.drc_edit_waives || c.drc_waives.is_some() {
            return Err(Error::input("--floe-reviewer is read-only; do not combine it with --drc-reviewer, --drc-edit-waives or --drc-waives"));
        }
    }
    if c.drc_edit_waives && c.drc_reviewer.is_none() {
        return Err(Error::input("--drc-edit-waives requires --drc-reviewer"));
    }
    if c.sources.len() > 32 {
        return Err(Error::input("view accepts at most 32 sources"));
    }
    if c.sources.is_empty()
        && (c.drc.is_some() || c.levels.is_some() || c.mode != "level" || c.initial != json!({}))
    {
        return Err(Error::input("display/DRC/level options require a source"));
    }
    if c.roots.len() > 32 {
        return Err(Error::input("too many approved roots"));
    }
    // argparse validates each integer's syntax, but cmd_view checks only the
    // final value's sign/conflicts. A valid negative followed by zero is OK.
    if stream_sign == Some(NumberSign::Less) {
        return Err(Error::input("--stream-kb must be >= 0"));
    }
    if (refinement_off || c.perf_baseline) && stream_sign == Some(NumberSign::Greater) {
        return Err(Error::input(
            "--refinement off (including --perf-baseline) conflicts with nonzero --stream-kb",
        ));
    }
    // The final stream=0 overrides on regardless of ordering. A positive
    // value preserves the round environment; its magnitude is not a budget.
    c.direct_final = refinement_off || stream_sign == Some(NumberSign::Equal);
    // Legacy cmd_view treats effective "on" exactly like omission: an
    // existing owner keeps its own round environment. Only effective off
    // introduces a construction option and requires an independent owner.
    c.independent |= refinement_off;
    if c.perf_baseline {
        c.frame_cache = false;
        c.direct_final = true;
        c.initial["frames"] = json!(false);
        c.initial["labels"] = json!(false);
    }
    Ok(c)
}
fn stream_kb_sign(text: &str) -> Result<NumberSign> {
    // Only zero/nonzero matters to the Rust adapter. Do not overflow or invent
    // a u64 ceiling for legacy decimal values whose magnitude is never used.
    // Accept signed ASCII decimal with Python's between-digit separators.
    let text = text.trim();
    let digits = text.strip_prefix(['-', '+']).unwrap_or(text);
    let mut zero = true;
    let mut digit = false;
    for byte in digits.bytes() {
        if byte.is_ascii_digit() {
            zero &= byte == b'0';
            digit = true;
        } else if byte == b'_' && digit {
            digit = false;
        } else {
            return Err(Error::input("--stream-kb requires a decimal integer"));
        }
    }
    if !digit {
        return Err(Error::input("--stream-kb requires a decimal integer"));
    }
    Ok(if zero {
        NumberSign::Equal
    } else if text.starts_with('-') {
        NumberSign::Less
    } else {
        NumberSign::Greater
    })
}
fn startup_depth(text: &str) -> Result<String> {
    if text == "full" {
        return Ok(text.into());
    }
    let text = text.trim();
    let digits = text.strip_prefix(['-', '+']).unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::input("depth must be full or an integer"));
    }
    let digits = digits.trim_start_matches('0');
    Ok(if text.starts_with('-') || digits.is_empty() {
        "0".into()
    } else if digits.len() > 3 || (digits.len() == 3 && digits >= "999") {
        "full".into()
    } else {
        digits.into()
    })
}
fn startup_labels(body: &Value) -> bool {
    body["frames"].as_bool().unwrap_or(true) && body["labels"].as_bool().unwrap_or(true)
}
fn startup_body(mut body: Value, deck: bool, drc: bool) -> Value {
    if body.get("depth").is_none() {
        body["depth"] = json!(if deck || drc || body.get("navigation").is_some() {
            "full"
        } else {
            "0"
        });
    }
    let frames = body["frames"].as_bool().unwrap_or(true);
    body["frames"] = json!(frames);
    body["labels"] = json!(!deck && startup_labels(&body));
    body
}
fn startup_levels(
    explicit: Option<BTreeSet<i64>>,
    deck: bool,
    count: usize,
    policy: Option<&str>,
) -> Result<(Option<BTreeSet<i64>>, bool)> {
    if explicit.is_some() || !deck {
        return Ok((explicit, false));
    }
    let policy = policy.unwrap_or("ask").trim();
    if policy.eq_ignore_ascii_case("all") {
        return Ok((None, false));
    }
    if policy.is_empty() || policy.eq_ignore_ascii_case("ask") {
        return Ok((None, count > 1));
    }
    Ok((
        Some(
            parse_levels(policy)
                .map_err(|_| Error::input("FLOE_JOBDECK_LEVELS must be all, ask or N,N..."))?,
        ),
        false,
    ))
}
fn number(s: &str, min: u64, max: u64, key: &str) -> Result<u64> {
    s.parse::<u64>()
        .ok()
        .filter(|v| *v >= min && *v <= max)
        .ok_or_else(|| Error::input(format!("{key} must be {min}..{max}")))
}

pub(crate) struct SessionFile {
    pub(crate) directory: PathBuf,
    pub(crate) path: PathBuf,
    identity: Option<(u64, u64)>,
}
impl SessionFile {
    pub(crate) fn create(explicit: Option<PathBuf>, value: &Value) -> Result<Self> {
        let root = fs::canonicalize(std::env::temp_dir())?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::input("invalid clock"))?
            .as_nanos();
        let directory = root.join(format!("floe-web-{}-{stamp}", std::process::id()));
        DirBuilder::new().mode(0o700).create(&directory)?;
        let mut owned = Self {
            path: directory.join("session.json"),
            directory,
            identity: None,
        };
        if let Some(p) = explicit {
            owned.path = cache::absolute(&p)?;
        }
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&owned.path)?;
        let meta = file.metadata()?;
        owned.identity = Some((meta.dev(), meta.ino()));
        file.write_all(serde_json::to_string(value).unwrap().as_bytes())?;
        file.sync_all()?;
        Ok(owned)
    }
}
impl Drop for SessionFile {
    fn drop(&mut self) {
        // Remove only this created inode, never a user replacement/symlink.
        if fs::symlink_metadata(&self.path).is_ok_and(|m| Some((m.dev(), m.ino())) == self.identity)
        {
            let _ = fs::remove_file(&self.path);
        }
        let _ = fs::remove_dir(&self.directory);
    }
}
// Only the trusted launcher selects a source/cache and derived review names.
// Admit metadata first; no arbitrary paths, repair, or implicit indexing.
fn readonly_selection(
    resources: &Arc<Resources>,
    scope: &AccessScope,
    path: &std::path::Path,
    reviewer: &str,
    stop: &AtomicUsize,
) -> Result<floe_app_core::drc::ReadSelection> {
    let path = scope.check(path)?;
    let mut files = vec![path.clone()];
    if !floe_app_core::drc::is_packed_source(&path)? {
        let mut candidate = path.as_os_str().to_owned();
        candidate.push(".ice");
        files.push(scope.check(&PathBuf::from(candidate))?);
    }
    let _permit = resources.drc(files)?;
    floe_app_core::drc::select_review(&path, reviewer, stop)
}
pub fn run(c: Command, cancelled: &Arc<AtomicUsize>) -> Result<i32> {
    if c.help {
        println!("{HELP}");
        return Ok(0);
    }
    // Claim/forward before Firefox or native discovery. A stale/busy owner is
    // an explicit error, never permission to create a second default instance.
    let owner = if c.independent {
        None
    } else {
        match handoff::claim()? {
            floe_app_core::instance::Claim::Owner(owner) => Some(owner),
            floe_app_core::instance::Claim::Running(endpoint) => {
                return handoff::forward(&endpoint, &c, cancelled)
            }
        }
    };
    let firefox = if c.no_open {
        None
    } else {
        Some(floe_app_core::browser::discover(c.firefox.as_deref())?)
    };
    let mut options = RenderOptions::local()?;
    if let Some(n) = c.jobs {
        options.decode_jobs = n;
    }
    if let Some(n) = c.raster {
        options.raster_jobs = n;
    }
    if let Some(n) = c.budget {
        options.budget_mb = n;
    }
    if let Some(v) = c.raw {
        options.raw = v;
    }
    if c.direct_final {
        options.round_pages = 1 << 30;
    }
    options.debug = c.render_debug;
    let resources = Resources::new(Limits::default())?;
    drop(resources.render(&options)?);
    let mut drc_roots = c.roots.clone();
    let mut roots = c.roots;
    for source in &c.sources {
        roots.push(
            cache::absolute(source)?
                .parent()
                .ok_or_else(|| Error::input("source has no parent"))?
                .to_owned(),
        );
    }
    for path in c
        .drc
        .iter()
        .chain(c.drc_waives.iter())
        .chain(c.drc_rules.iter())
    {
        drc_roots.push(
            cache::absolute(path)?
                .parent()
                .ok_or_else(|| Error::input("DRC path has no parent"))?
                .to_owned(),
        );
    }
    roots.sort();
    roots.dedup();
    let browse_roots = if roots.is_empty() {
        vec![std::env::current_dir()?]
    } else {
        let mut browse_roots = roots.clone();
        // Match GTK's initial directory when a source was supplied.
        if let Some(source) = c.sources.first() {
            let parent = cache::absolute(source)?
                .parent()
                .expect("validated source parent")
                .to_owned();
            browse_roots.retain(|p| p != &parent);
            browse_roots.insert(0, parent);
        }
        browse_roots
    };
    let scope = if c.sources.is_empty() {
        None
    } else {
        Some(AccessScope::new(&roots)?)
    };
    // An explicit review pack must not broaden a deck's TC dependency roots.
    // Only --root grants extra roots to both registrations.
    let drc_scope = if c.drc.is_some() {
        drc_roots.sort();
        drc_roots.dedup();
        Some(AccessScope::new(&drc_roots)?)
    } else {
        None
    };
    let read_selection = c
        .read_reviewer
        .as_ref()
        .map(|tag| {
            readonly_selection(
                &resources,
                drc_scope.as_ref().unwrap(),
                c.drc.as_ref().unwrap(),
                tag,
                cancelled,
            )
        })
        .transpose()?;
    let read_sidecars = read_selection.as_ref().is_some_and(|s| s.targets.is_some());
    if let Some(s) = &read_selection {
        if let Some(warning) = &s.warning {
            eprintln!("[floe2-web] {warning}");
        }
        eprintln!(
            "[floe2-web] read-only reviewer: {}; no directory browsing or review writes",
            if read_sidecars {
                "ICE with derived adjacent/legacy sidecars"
            } else {
                "ASCII fallback; notes/waives unavailable without a current ICE cache"
            }
        );
    }
    let sources = c
        .sources
        .iter()
        .map(|p| {
            RegisteredSource::register(
                Arc::clone(scope.as_ref().expect("source scope")),
                p,
                cancelled,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let level_env = std::env::var("FLOE_JOBDECK_LEVELS").ok();
    let label_preference = startup_labels(&c.initial);
    let window_seed: floe_web::view::PatchDto =
        serde_json::from_value(startup_body(c.initial.clone(), false, c.drc.is_some()))
            .map_err(|_| Error::input("invalid initial display"))?;
    let preferences = if let Some(first) = sources.first() {
        let (levels, confirm) = startup_levels(
            c.levels,
            first.deck,
            first.levels.len(),
            level_env.as_deref(),
        )?;
        first.validate_levels(levels.as_ref())?;
        if !first.deck && c.mode != "level" {
            return Err(Error::input("chip/source-layer mode requires a jobdeck"));
        }
        Some((
            levels,
            confirm,
            startup_body(c.initial, first.deck, c.drc.is_some()),
        ))
    } else {
        None
    };
    let indexer = Indexer::discover(&Discovery::local()?)?;
    let service = Service::start_configured(
        sources,
        Arc::clone(&resources),
        options.clone(),
        indexer.clone(),
        ControllerOptions {
            margin_prefetch: c.frame_cache,
            frame_cache: c.frame_cache,
        },
    )?;
    service.seed_window_display(window_seed.core().map_err(Error::input)?)?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, c.port))?;
    listener.set_nonblocking(true)?;
    let (mut gate, secret) = if let Some((levels,confirm_levels,initial)) = preferences {
        let request = json!({"kind":"open","seq":"1","source_id":service.catalog()["sources"][0]["source_id"],"mode":c.mode,
            "levels":handoff::levels_json(levels),"body":initial,"label_preference":label_preference});
        Gateway::with_startup_options(listener.local_addr()?,Arc::clone(&service),request,confirm_levels)
    } else { Gateway::with_service(listener.local_addr()?,Arc::clone(&service)) }.map_err(Error::input)?;
    Gateway::attach_build(&mut gate, crate::selfcheck::build_info()).map_err(Error::input)?;
    if c.dump {
        Gateway::enable_display_dump(&mut gate).map_err(Error::input)?;
    }
    let notices = match crate::selfcheck::notice_catalog(cancelled) {
        Ok(Some(c)) => floe_web::about::Notices::Ready(Arc::new(c)),
        Ok(None) => floe_web::about::Notices::NotPackaged,
        Err(e) => {
            eprintln!("[floe2-web] portable notices unavailable: {e}");
            floe_web::about::Notices::Unavailable
        }
    };
    floe_app_core::check_cancelled(cancelled)?;
    Gateway::attach_notices(&mut gate, notices).map_err(Error::input)?;
    if let Some(path) = &c.drc {
        // The explicit write opt-in also reads that fixed reviewer's existing
        // file on ICE reopen. It never discovers another reviewer or a pack.
        let default_waives = if c.drc_edit_waives && c.drc_waives.is_none() {
            let mut header = [0; 8];
            let ice = match fs::File::open(path)?.read_exact(&mut header) {
                Ok(()) => &header == floe_app_core::drc::MAGIC,
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => false,
                Err(e) => return Err(e.into()),
            };
            let target = floe_app_core::drc::review::store::paths(
                path,
                c.drc_reviewer.as_deref().unwrap(),
                floe_app_core::drc::review::store::Kind::Waives,
            )?[0]
                .clone();
            if ice && target.try_exists()? {
                Some(target)
            } else {
                None
            }
        } else {
            None
        };
        let catalog = service.catalog();
        let source_id = catalog["sources"][0]["source_id"].as_str().unwrap();
        let drc_scope = drc_scope.expect("DRC-specific registration scope");
        let drc = if let Some(selected) = read_selection {
            floe_web::drc::Service::start_readonly_review(
                &resources,
                drc_scope,
                selected,
                c.drc_rules.as_deref(),
                source_id,
                c.read_reviewer.as_deref().unwrap(),
            )?
        } else {
            floe_web::drc::Service::start_with_rules(
                &resources,
                drc_scope,
                path,
                c.drc_waives.as_deref().or(default_waives.as_deref()),
                c.drc_rules.as_deref(),
                source_id,
            )?
        };
        // Check combined capacity before publishing a URL or starting a browser.
        // The actual view acquires its own reservation when opened by the UI.
        drop(resources.render(&options)?);
        let registry = floe_web::drc::Registry::with_builds(drc, indexer)?;
        Gateway::attach_drc_registry(&mut gate, registry).map_err(Error::input)?;
    }
    let url = format!("{}/#bootstrap={}", gate.origin(), secret.expose());
    let session = SessionFile::create(
        c.session_file,
        &json!({"url":url,"origin":gate.origin(),"bundle":BUNDLE,"pid":std::process::id()}),
    )?;
    if let Some(tag) = &c.drc_reviewer {
        Gateway::enable_drc_review(
            &mut gate,
            tag,
            std::slice::from_ref(&session.path),
            std::slice::from_ref(&session.directory),
            c.drc_edit_waives,
        )
        .map_err(Error::input)?;
    }
    if let Some(tag) = c.read_reviewer.as_ref().filter(|_| read_sidecars) {
        Gateway::enable_drc_readonly_review(
            &mut gate,
            tag,
            std::slice::from_ref(&session.path),
            std::slice::from_ref(&session.directory),
        )
        .map_err(Error::input)?;
    }
    if std::env::var_os("FLOE_FILL_EDIT").is_some_and(|v| !v.is_empty()) {
        Gateway::enable_fill_slot_edit(&mut gate).map_err(Error::input)?;
        Gateway::enable_design_defaults(
            &mut gate,
            std::slice::from_ref(&session.path),
            std::slice::from_ref(&session.directory),
        )
        .map_err(Error::input)?;
    }
    let launches = floe_web::launch::Launches::new();
    if owner.is_some() {
        Gateway::attach_launches(&mut gate, Arc::clone(&launches)).map_err(Error::input)?;
    }
    Gateway::enable_browse(&mut gate, &browse_roots, &resources).map_err(Error::input)?;
    // Verify the combined reservation before advertising a usable workspace.
    drop(resources.render(&options)?);
    let mut instance = owner
        .map(|owner| handoff::Runtime::start(owner, Arc::clone(&service), launches))
        .transpose()?;
    let mut browser = firefox
        .map(|path| floe_app_core::browser::Browser::start(&path, &session.directory, &url))
        .transpose()?;
    eprintln!("[floe2-web] local workspace: {}", gate.origin());
    eprintln!(
        "[floe2-web] private session link: {} (one use, expires in 120s)",
        session.path.display()
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut browser_error = None;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::from_std(listener)?;
        transport::serve(listener, gate, async {
            while cancelled.load(Ordering::Relaxed) == 0 && !service.is_finished() {
                if instance.as_ref().is_some_and(handoff::Runtime::is_finished) {
                    break;
                }
                if let Some(browser) = &browser {
                    match browser.exited() {
                        Ok(true) => break,
                        Err(e) => {
                            browser_error = Some(e);
                            break;
                        }
                        Ok(false) => (),
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
    })?;
    if let Some(instance) = &mut instance {
        instance.close()?;
    }
    if let Some(browser) = &mut browser {
        browser.close()?;
    }
    if let Some(error) = browser_error {
        return Err(error);
    }
    drop(session);
    let signal = cancelled.load(Ordering::Relaxed);
    Ok(if signal == 0 { 0 } else { 128 + signal as i32 })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bare_source_dispatch_is_the_same_parser_without_filesystem_guessing() {
        use std::ffi::OsString;
        for words in [
            vec!["한국 설계.oas", "--goto", "5,6,300", "--thin", "keep"],
            vec!["--detail", "high", "--depth", "7", "PATTERN01.TE"],
            vec!["mask.JB", "--level", "2,1", "--mode", "chip"],
            vec!["--multi", "--no-open"],
            vec!["--jobs", "2", "--", "-mask", "index"],
            vec!["./render"],
        ] {
            let crate::Cli::View(shorthand) =
                crate::parse(words.iter().map(OsString::from)).unwrap()
            else {
                panic!()
            };
            let explicit: Vec<_> = std::iter::once("view")
                .chain(words.iter().copied())
                .map(str::to_owned)
                .collect();
            let expected = parse(&explicit).unwrap();
            // Include all launch fields, not only the displayed source name.
            assert_eq!(
                format!("{shorthand:?}"),
                format!("{expected:?}"),
                "{words:?}"
            );
        }
        assert!(crate::parse([OsString::from("index")]).is_err());
        assert!(matches!(
            crate::parse([OsString::from("selfcheck")]).unwrap(),
            crate::Cli::SelfCheck(_)
        ));
        for words in [
            vec!["--version", "a.oas"],
            vec!["--help", "a.oas"],
            vec!["a.oas", "--force"],
            vec!["--detail", "high"],
            vec!["a.oas", "--bogus"],
        ] {
            assert!(
                crate::parse(words.iter().map(OsString::from)).is_err(),
                "{words:?}"
            );
        }
    }
    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }
    #[test]
    fn direct_final_alias_and_debug_are_independent_process_options() {
        for tail in [
            "--stream-kb 0",
            "--stream-kb=0",
            "--stream-kb 0 --refinement off",
            "--stream-kb 0 --refinement on",
            "--refinement on --stream-kb 0",
        ] {
            let c = parse(&args(&format!("view {tail}"))).unwrap();
            assert!(c.direct_final && c.independent);
            assert!(!c.render_debug);
            assert!(c.frame_cache); // Direct-final is not the performance baseline.
            assert_eq!(c.initial, json!({}));
        }
        let c = parse(&args("view --render-debug")).unwrap();
        assert!(c.render_debug && c.independent);
        assert!(!c.direct_final);
        assert!(!parse(&args("view")).unwrap().render_debug);
        let c = parse(&args("view --dump")).unwrap();
        assert!(c.dump && c.independent && !c.render_debug && !c.direct_final);
        assert_eq!(c.initial, json!({})); // Not forwarded as a display patch.
        assert!(!parse(&args("view")).unwrap().dump);
        for tail in [
            "--stream-kb",
            "--stream-kb -1",
            "--stream-kb NaN",
            "--stream-kb 0.0",
            "--refinement off --stream-kb 8",
            "--perf-baseline --stream-kb 8",
            "--render-debug=false",
            "--dump=false",
        ] {
            assert!(parse(&args(&format!("view {tail}"))).is_err(), "{tail}");
        }
    }
    #[test]
    fn stream_compatibility_uses_only_the_last_integer_sign() {
        for (tail, final_only) in [
            ("--stream-kb 1", false),
            ("--stream-kb=65536", false),
            ("--stream-kb +1_024", false),
            ("--stream-kb 184467440737095516160", false),
            ("--stream-kb -0", true),
            ("--stream-kb 0 --stream-kb 8", false),
            ("--stream-kb 8 --stream-kb 0", true),
            ("--stream-kb -8 --stream-kb 0", true),
            ("--stream-kb -8 --stream-kb 2", false),
            ("--stream-kb 8 --refinement off --refinement on", false),
            ("--refinement off --stream-kb 8 --stream-kb 0", true),
            ("--perf-baseline --stream-kb 8 --stream-kb 0", true),
        ] {
            let c = parse(&args(&format!("view {tail}"))).unwrap();
            assert!(c.independent, "{tail}");
            assert_eq!(c.direct_final, final_only, "{tail}");
        }
        for tail in [
            "--stream-kb 0 --stream-kb -8",
            "--stream-kb 8 --refinement off",
            "--stream-kb 8 --perf-baseline --refinement on",
            "--stream-kb NaN --stream-kb 0",
            "--stream-kb +_8",
            "--stream-kb 8_",
            "--stream-kb 8__0",
            "--stream-kb 0x10",
        ] {
            assert!(parse(&args(&format!("view {tail}"))).is_err(), "{tail}");
        }
        assert_eq!(stream_kb_sign(" +00_00 ").unwrap(), NumberSign::Equal);
        assert!(stream_kb_sign("").is_err());
    }
    #[test]
    #[ignore = "requires generated GTK CLI stream/refinement oracle"]
    fn gtk_stream_policy_oracle() {
        let cases: Value = serde_json::from_slice(
            &fs::read(std::env::var("FLOE_STREAM_ORACLE").unwrap()).unwrap(),
        )
        .unwrap();
        let cases = cases.as_array().unwrap();
        for case in cases {
            let argv: Vec<String> = serde_json::from_value(case["argv"].clone()).unwrap();
            let result = parse(&argv);
            if case["want"].is_null() {
                assert!(result.is_err(), "{argv:?}");
            } else {
                let c = result.unwrap_or_else(|e| panic!("{argv:?}: {e}"));
                assert_eq!(
                    c.direct_final,
                    case["want"]["direct_final"].as_bool().unwrap(),
                    "{argv:?}"
                );
                assert_eq!(
                    c.independent,
                    case["want"]["independent"].as_bool().unwrap(),
                    "{argv:?}"
                );
            }
        }
        println!("GTK STREAM POLICY: ALL OK ({} cases)", cases.len());
    }
    #[test]
    fn refinement_on_preserves_environment_and_legacy_precedence() {
        for (tail, direct_final) in [
            ("--refinement on", false),
            ("--refinement=on", false),
            ("--refinement off --refinement on", false),
            ("--refinement on --refinement off", true),
            ("--refinement on --perf-baseline", true),
            ("--perf-baseline --refinement on", true),
        ] {
            let c = parse(&args(&format!("view {tail}"))).unwrap();
            assert_eq!(c.direct_final, direct_final, "{tail}");
            assert_eq!(c.independent, direct_final, "{tail}");
            assert!(!c.render_debug);
        }
        assert!(parse(&args("view --refinement bad")).is_err());
    }
    #[test]
    fn unmigrated_options_explain_their_actual_boundary() {
        for (tail, explanation) in [
            ("--hairline 0", "legacy KLayout planner"),
            ("--thin-um 0", "not an equivalent frame control"),
            ("--stream-target-ms 500", "unused by Rust renderd"),
            ("--lod off", "not sent to Rust renderd"),
        ] {
            let e = parse(&args(&format!("view missing.oas {tail}"))).unwrap_err();
            assert!(e.to_string().contains(explanation), "{e}");
        }
    }
    #[test]
    fn startup_depth_display_and_levels_follow_legacy_policy() {
        for (value, want) in [
            ("-12", "0"),
            ("+0099", "99"),
            ("998", "998"),
            ("999", "full"),
            ("99999999999999999999999999", "full"),
            ("full", "full"),
        ] {
            assert_eq!(startup_depth(value).unwrap(), want);
        }
        for value in ["", "+", "1.0", "NaN", "１", "1e3"] {
            assert!(startup_depth(value).is_err());
        }
        for (tail, depth, frames, labels, baseline) in [
            ("", "0", true, true, false),
            ("--goto 1,2", "full", true, true, false),
            ("--goto 1,2 --depth 0", "0", true, true, false),
            ("--drc result.db", "full", true, true, false),
            ("--frames off --labels on", "0", false, false, false),
            ("--frames=on --labels=off", "0", true, false, false),
            (
                "--perf-baseline --frames on --labels on --frame-cache on --depth full",
                "full",
                false,
                false,
                true,
            ),
        ] {
            let c = parse(&args(&format!("view a.oas {tail}"))).unwrap();
            assert_eq!(c.direct_final, baseline);
            if baseline {
                assert!(!c.frame_cache);
            }
            let body = startup_body(c.initial, false, c.drc.is_some());
            assert_eq!(body["depth"], depth, "{tail}");
            assert_eq!(body["frames"], frames);
            assert_eq!(body["labels"], labels);
        }
        assert_eq!(startup_body(json!({}), true, false)["labels"], false);
        assert_eq!(startup_body(json!({}), true, false)["depth"], "full");
        assert_eq!(startup_levels(None, true, 2, None).unwrap(), (None, true));
        assert_eq!(startup_levels(None, true, 1, None).unwrap(), (None, false));
        assert_eq!(
            startup_levels(None, true, 2, Some("all")).unwrap(),
            (None, false)
        );
        assert_eq!(
            startup_levels(None, false, 2, Some("bad")).unwrap(),
            (None, false)
        );
        let levels = Some([2, 3].into());
        assert_eq!(
            startup_levels(levels.clone(), true, 3, Some("bad")).unwrap(),
            (levels, false)
        );
        assert_eq!(
            startup_levels(None, true, 3, Some("2,3")).unwrap(),
            (Some([2, 3].into()), false)
        );
        assert!(startup_levels(None, true, 3, Some("bad")).is_err());
        for tail in [
            "--goto 1",
            "--goto 1,2,0",
            "--goto NaN,2",
            "--refinement bad",
            "--frames=bad",
            "--labels=bad",
        ] {
            assert!(parse(&args(&format!("view a.oas {tail}"))).is_err());
        }
        assert!(
            parse(&args("view a.oas --refinement off"))
                .unwrap()
                .direct_final
        );
        assert_eq!(
            parse(&args("view a.jb --mode layer")).unwrap().mode,
            "layer"
        );
    }
    #[test]
    #[ignore = "run tools/validate_web_startup.py for the GTK source oracle"]
    fn gtk_startup_oracle() {
        use floe_app_core::view::Viewport;
        let path = std::env::var_os("FLOE_STARTUP_ORACLE").expect("startup oracle");
        let cases: Vec<Value> = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert!(cases.len() >= 120);
        for case in &cases {
            let argv: Vec<String> = serde_json::from_value(case["argv"].clone()).unwrap();
            let c = parse(&argv).unwrap();
            let body = startup_body(c.initial, case["deck"].as_bool().unwrap(), c.drc.is_some());
            for key in ["depth", "frames", "labels"] {
                assert_eq!(body[key], case["want"][key], "{argv:?}: {key}");
            }
            let pixels: [u32; 2] = serde_json::from_value(case["pixels"].clone()).unwrap();
            let bbox: [f64; 4] = serde_json::from_value(case["bbox"].clone()).unwrap();
            let dbu = case["dbu"].as_f64().unwrap();
            let mut viewport = Viewport::fit(bbox, pixels[0], pixels[1]).unwrap();
            let patch: floe_web::view::PatchDto = serde_json::from_value(body).unwrap();
            if let Some(nav) = patch.core().unwrap().navigation {
                viewport = viewport.navigate(nav, bbox, dbu).unwrap();
            }
            for (got, want) in viewport
                .bbox
                .iter()
                .zip(case["want"]["bbox"].as_array().unwrap())
            {
                let want = want.as_f64().unwrap();
                assert!(
                    (got - want).abs() <= 1e-8 * want.abs().max(1.),
                    "{argv:?}: {viewport:?} vs {}",
                    case["want"]["bbox"]
                );
            }
        }
        println!(
            "GTK STARTUP: ALL OK ({} depth/display/fit/goto cases)",
            cases.len()
        );
    }
    #[test]
    fn initial_settings_form_one_bounded_patch() {
        let c=parse(&args("view source.oas --goto -10.9375,20,700 --depth 99 --detail high --thin keep --jobs 8 --raster-jobs 4 --no-open --label-font-px 18 --no-frames --labels")).unwrap();
        assert_eq!(c.initial["navigation"]["center_um"][0], "-10.9375");
        assert_eq!(c.initial["navigation"]["width_um"], "700");
        assert_eq!(c.initial["depth"], "99");
        assert_eq!(c.initial["detail"], "high");
        assert!(c.no_open);
        assert_eq!(c.initial["font_px"], 18);
        assert_eq!(c.initial["labels"], true);
        assert_eq!(c.initial["frames"], false);
        for s in [
            "view --goto 0,0,100",
            "view a --jobs 0",
            "view a --raster-jobs 17",
            "view a --goto 0,0,-1",
            "view a --thin bad",
            "view a --listen 0.0.0.0",
            "view a --no-open=yes",
            "view a --label-font-px 97",
            "view a --label-font-px 5",
            "view a --drc-waives review",
            "view a --drc-rules rules.json",
            "view a --drc-reviewer test",
            "view a --drc b --drc-reviewer ../escape",
            "view a --drc-edit-waives",
            "view a --drc b --drc-edit-waives",
            "view a --drc b --drc-reviewer fixed --drc-edit-waives=true",
        ] {
            assert!(parse(&args(s)).is_err(), "{s}");
        }
        assert!(parse(&args(
            "view a --drc results.ice --drc-rules rules.json --no-open"
        ))
        .unwrap()
        .drc_rules
        .is_some());
        assert!(parse(&args("view --help")).unwrap().help);
        let c = parse(&args("view a --drc b.ice --drc-waives side")).unwrap();
        assert!(c.drc_reviewer.is_none());
        assert!(!c.drc_edit_waives);
        assert!(
            parse(&args(
                "view a --drc b.ice --drc-reviewer fixed --drc-edit-waives"
            ))
            .unwrap()
            .drc_edit_waives
        );
        assert_eq!(
            parse(&args("view a --drc b.ice --drc-reviewer fixed-owner"))
                .unwrap()
                .drc_reviewer
                .as_deref(),
            Some("fixed-owner")
        );
        assert_eq!(c.drc, Some(PathBuf::from("b.ice")));
        assert_eq!(c.drc_waives, Some(PathBuf::from("side")));
    }
    #[test]
    fn readonly_reviewer_is_not_a_write_grant_or_forwarded_display_patch() {
        let c = parse(&args("view a.oas --drc review.ice --floe-reviewer fixed")).unwrap();
        assert_eq!(c.read_reviewer.as_deref(), Some("fixed"));
        assert!(c.drc_reviewer.is_none() && !c.drc_edit_waives && c.independent);
        for tail in [
            "--drc-reviewer fixed",
            "--drc-reviewer other",
            "--drc-waives any",
            "--drc-edit-waives",
        ] {
            assert!(parse(&args(&format!(
                "view a --drc review.ice --floe-reviewer fixed {tail}"
            )))
            .is_err());
        }
        for tail in ["", "--drc review.ice --floe-reviewer ../escape"] {
            assert!(parse(&args(&format!("view a --floe-reviewer fixed {tail}"))).is_err());
        }
    }
    #[test]
    fn empty_window_and_process_options_have_explicit_instance_policy() {
        assert!(parse(&args("view")).unwrap().sources.is_empty());
        assert!(!parse(&args("view --no-open")).unwrap().independent);
        assert!(parse(&args("view --multi --no-open")).unwrap().independent);
        let baseline = parse(&args("view --perf-baseline")).unwrap();
        assert!(baseline.sources.is_empty() && baseline.independent && baseline.direct_final);
        assert!(!baseline.frame_cache);
        assert_eq!(baseline.initial["frames"], false);
        assert_eq!(baseline.initial["labels"], false);
        for options in [
            "--jobs 2",
            "--raster-jobs 1",
            "--budget-mb 512",
            "--png",
            "--raw",
            "--frame-cache off",
            "--refinement off",
            "--perf-baseline",
            "--port 0",
            "--session-file /tmp/session.json",
            "--firefox /tmp/firefox",
            "--drc /tmp/results.db",
        ] {
            assert!(
                parse(&args(&format!("view /tmp/a.oas {options}")))
                    .unwrap()
                    .independent,
                "{options}"
            );
        }
        assert!(
            !parse(&args(
                "view a.oas --goto 1,2,100 --thin auto --depth 99 --detail high"
            ))
            .unwrap()
            .independent
        );
    }
}
