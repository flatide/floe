//! Headless named captures, one native worker, per-artifact atomic commit.
use crate::{
    annotations::{self, Document},
    artifact::{self, StagedArtifact},
    check_cancelled,
    dataset::Dataset,
    jobdeck::{
        chips::{ChipFrame, ChipTable, Selection},
        plan::Analysis,
        window::DeckWindow,
    },
    render::{budget_fit, budget_fit_brief, require_complete, RenderOptions, RenderSession},
    shots::{
        batch::{Capture, NamedCapture, TILE_NAMES},
        mosaic::{self, Mosaic},
    },
    Error, ErrorKind, Result,
};
use floe_worker_client::{Distinct, FrameFormat, Layers, Style};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{atomic::AtomicUsize, Arc},
    time::Instant,
};

pub struct ExportOptions {
    pub out: PathBuf,
    pub report: Option<PathBuf>,
    pub batch: bool,
    pub input: Option<PathBuf>,
    /// `--legend` (2026-10-13): each image carries the legend of the layers
    /// it draws (crate::legend) as flateyes metadata
    pub legend: bool,
}
struct Plan<'a> {
    named: &'a NamedCapture,
    tiles: Vec<crate::shots::Shot>,
    display: PathBuf,
    target: PathBuf,
    keep: Vec<PathBuf>,
    /// The skipped placements this capture lacks (indices into the ledger).
    lacks: Vec<usize>,
    /// the chips the capture names, and the default region they frame
    chips: Option<Selection>,
    /// `--in-chip`: the placement whose chip coordinates the region is in
    in_chip: Option<ChipFrame>,
    region: [f64; 4],
    /// the tiles' frame (Shot::frame): the size drawn, and the size asked
    /// when the 16 Mpx limit reduced it - the report's `pixel_requested`
    frame: crate::shots::Fitted,
}
/// What a run of captures drew: whether every image is whole, and what
/// they lack - the skipped placements of the chips some shot had on, the
/// pages over the decode budget, and a jobdeck's passes of light sources
/// whose occupancy summary is not built yet (left empty; DIRECT_READ_PLAN
/// §5.5 - `floe2 index DECK.jb` adds it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Summary {
    pub complete: bool,
    pub lacking: usize,
    pub over_budget: u64,
    pub summary_building: u64,
}
/// The ledger records a capture drawing `layers` lacks (floe.shots
/// plan_shots, jobdeck.chips ChipTable.skipped_in): every record with every
/// layer on, else those of the chips on - a chip is a deck view row with a
/// source, keyed by its layer/datatype, and a record no row names counts, to
/// be safe. A layout skips nothing.
fn lacks(dataset: &Dataset, layers: &Layers) -> Vec<usize> {
    let skipped = dataset.skipped();
    let on: BTreeSet<(i64, i64)> = match layers {
        Layers::All => return (0..skipped.len()).collect(),
        Layers::None => BTreeSet::new(),
        Layers::Only(keys) => keys
            .iter()
            .map(|&(l, d)| (i64::from(l), i64::from(d)))
            .collect(),
    };
    let Dataset::Deck(deck) = dataset else {
        return Vec::new();
    };
    let rows: BTreeMap<(i64, String), (i64, i64)> = deck
        .metadata
        .layers
        .iter()
        .filter(|l| !l.jobdeck_head)
        .filter_map(|l| {
            let tc = crate::jobdeck::view::normalize_tc(l.jobdeck_source.as_deref()?);
            Some(((l.layer, tc), (l.layer, l.datatype)))
        })
        .collect();
    skipped
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            rows.get(&(r.idx, crate::jobdeck::view::normalize_tc(&r.tc)))
                .is_none_or(|key| on.contains(key))
        })
        .map(|(i, _)| i)
        .collect()
}
/// How long a capture's frame may take: FLOE_RENDER_TIMEOUT_S (seconds,
/// positive), else a week - the frame is awaited, not abandoned
pub fn capture_timeout() -> std::time::Duration {
    std::env::var("FLOE_RENDER_TIMEOUT_S")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s > 0.0)
        .map_or(std::time::Duration::from_secs(7 * 24 * 3600), std::time::Duration::from_secs_f64)
}

fn sidecars(path: &Path) -> Result<Vec<PathBuf>> {
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| Error::input("output filename must be UTF-8"))?;
    let stem = if filename.to_ascii_lowercase().ends_with(".png") {
        &filename[..filename.len() - 4]
    } else {
        filename
    };
    Ok(TILE_NAMES
        .iter()
        .map(|tag| path.with_file_name(format!("{stem}_{tag}.png")))
        .collect())
}

pub fn run(
    dataset: &Dataset,
    shots: &[NamedCapture],
    options: &ExportOptions,
    render: RenderOptions,
    cancelled: &Arc<AtomicUsize>,
    log: impl FnMut(&str),
) -> Result<bool> {
    run_summary(dataset, shots, options, render, cancelled, log).map(|s| s.complete)
}

/// A few skipped placements, for one log line (floe.shots `brief`).
fn brief(records: &[&crate::jobdeck::geom::Skipped]) -> String {
    let mut text = records
        .iter()
        .take(5)
        .map(|r| format!("CHIP {} level {} {} {}", r.chip, r.idx, r.tc, r.reason))
        .collect::<Vec<_>>()
        .join("; ");
    if records.len() > 5 {
        text.push_str(" ...");
    }
    text
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Which rows a deck capture's legend lists (`FLOE_LEGEND_IN_VIEW`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LegendRows {
    /// the rows a pixel of the picture shows, as the renderer says them
    /// (Frame::drawn; the default since 0.12.439 - user 2026-11-10: "실제
    /// 그려진 것만 레전드를 추가하도록")
    Drawn,
    /// `placed`: the rows with a placement in the frame (the rule of
    /// 0.12.438 - a chip placed there with no shape in the frame is listed)
    Placed,
    /// `off`: every row of the levels loaded (before 0.12.438)
    Every,
}

fn legend_rows() -> LegendRows {
    legend_rows_of(std::env::var("FLOE_LEGEND_IN_VIEW").ok().as_deref())
}

fn legend_rows_of(value: Option<&str>) -> LegendRows {
    match value.map(str::trim) {
        Some("off") => LegendRows::Every,
        Some("placed") => LegendRows::Placed,
        _ => LegendRows::Drawn,
    }
}

/// Whether a deck capture's rows are told apart by their colours
/// (`FLOE_DECK_DISTINCT_COLORS`; user 2026-11-11: "가능하면 칠해진 레이어의
/// 색이 겹치지 않아야 하는데 현재는 겹치기도 함 … 실제로 칠할때 색을
/// 결정하고 이전에 그린 색과 최대한 겹치지 않게 선택"): the renderer decides
/// a row's colour when it first paints - its own unless a row painted before
/// it has that one (DeckSnapshot::distinct). `off`: every row in the view's
/// colour, as before 0.12.449.
fn distinct_colors() -> bool {
    distinct_colors_of(std::env::var("FLOE_DECK_DISTINCT_COLORS").ok().as_deref())
}

fn distinct_colors_of(value: Option<&str>) -> bool {
    value.map(str::trim) != Some("off")
}

/// The room a picture's rows may take of a render command (its line is 64
/// KiB with the layers on and the rest): the fixed rows, `L/D,`, and what a
/// mosaic's later tile is told of the tiles before it, `L/D:RRGGBB,` a row.
/// About two thousand rows.
const DISTINCT_BYTES_MAX: usize = 24 * 1024;

/// What a picture's frames are asked (`base`: the deck's,
/// DeckSnapshot::distinct): of the rows whose colour is the layer
/// properties' those of a group (RowsBy::group: the rows that are one
/// colour) `placed` in the picture - a colour of a row the picture cannot
/// show need not be kept from the others. None: the rows would not fit the
/// command (`DISTINCT_BYTES_MAX`; a mosaic tells its later tiles a row of
/// every group the earlier ones painted, at most the groups placed) - the
/// picture is drawn in the view's colours.
fn distinct_for(
    base: &Distinct,
    placed: &BTreeSet<(u32, u32)>,
    mosaic: bool,
) -> Option<Distinct> {
    // a row of each group placed (the one with the longest key first met
    // would do as well: the room is an estimate)
    let mut groups: BTreeMap<(u32, u32), (u32, u32)> = BTreeMap::new();
    for &row in placed {
        groups.entry(base.by.group(row)).or_insert(row);
    }
    let fixed: Vec<(u32, u32)> = base
        .fixed
        .iter()
        .copied()
        .filter(|&row| groups.contains_key(&base.by.group(row)))
        .collect();
    let digits = |n: u32| n.checked_ilog10().unwrap_or(0) as usize + 1;
    let bytes = |row: &(u32, u32), more: usize| digits(row.0) + digits(row.1) + 2 + more;
    let mut need: usize = fixed.iter().map(|row| bytes(row, 0)).sum();
    if mosaic {
        need += groups.values().map(|row| bytes(row, 7)).sum::<usize>();
    }
    (need <= DISTINCT_BYTES_MAX).then(|| Distinct {
        fixed,
        ..base.clone()
    })
}

/// What a later tile of a picture is told of the tiles before it
/// (Distinct::taken): of the rows `painted` so far one of each group, with
/// its colour - the groups left as they are (`asked.fixed`) need no telling.
fn taken_of(asked: &Distinct, painted: &BTreeMap<(u32, u32), [u8; 4]>) -> Vec<((u32, u32), [u8; 4])> {
    let fixed: BTreeSet<(u32, u32)> = asked.fixed.iter().map(|&row| asked.by.group(row)).collect();
    let mut groups: BTreeMap<(u32, u32), ((u32, u32), [u8; 4])> = BTreeMap::new();
    for (&row, &color) in painted {
        let group = asked.by.group(row);
        if !fixed.contains(&group) {
            groups.entry(group).or_insert((row, color));
        }
    }
    groups.into_values().collect()
}

/// `distinct_for` of a shot's tiles: the rows placed are looked up only
/// when they matter - some row's colour is the layer properties', or the
/// picture is a mosaic.
fn picture_distinct(
    dataset: &Dataset,
    base: &Distinct,
    tiles: &[crate::shots::Shot],
    mosaic: bool,
) -> Result<Option<Distinct>> {
    if base.fixed.is_empty() && !mosaic {
        return Ok(Some(base.clone()));
    }
    let placed = placed_pairs(dataset, tiles)?.unwrap_or_default();
    Ok(distinct_for(base, &placed, mosaic))
}

/// The styles a picture's rows were painted with: `styles`, each row in the
/// colour the frames painted it in (`painted`, by row), and a row that did
/// not paint in the colour of the rows it is one colour with (`asked.by`: a
/// level view's head has its chips' colour) - unless its group is left as it
/// is (`asked.fixed`), or none of it painted: then its own.
fn painted_styles<'a>(
    styles: &'a [Style],
    asked: Option<&Distinct>,
    painted: &BTreeMap<(u32, u32), [u8; 4]>,
) -> Cow<'a, [Style]> {
    let Some(asked) = asked.filter(|_| !painted.is_empty()) else {
        return Cow::Borrowed(styles);
    };
    let groups: BTreeMap<(u32, u32), [u8; 4]> = taken_of(asked, painted)
        .into_iter()
        .map(|(row, color)| (asked.by.group(row), color))
        .collect();
    Cow::Owned(
        styles
            .iter()
            .map(|style| Style {
                color: painted
                    .get(&style.layer)
                    .or_else(|| groups.get(&asked.by.group(style.layer)))
                    .copied()
                    .unwrap_or(style.color),
                ..style.clone()
            })
            .collect(),
    )
}

/// Of a deck, the view's L/D pairs with a placement in the capture's frame -
/// the tiles' regions (a mosaic's four together), judged as the capture
/// window judges what it reads (DeckWindow::reaches: a touched edge counts,
/// an unknown extent always). The legend's rows under
/// `FLOE_LEGEND_IN_VIEW=placed` (the rule of 0.12.438; user 2026-11-10: "칩
/// 모드로 캡쳐할 경우 레전드에 해당 레벨 칩이 모두 나오는데 현재 캡쳐된
/// 화면에 있는 것들만 레전드에 포함되게"), and what stands in for a frame
/// that does not say what it shows. None: a layout - every row.
fn placed_pairs(
    dataset: &Dataset,
    tiles: &[crate::shots::Shot],
) -> Result<Option<BTreeSet<(u32, u32)>>> {
    let Dataset::Deck(deck) = dataset else {
        return Ok(None);
    };
    let window = DeckWindow::new(tiles.iter().map(|t| t.bbox.unwrap()))?;
    deck.placed_pairs(&window).map(Some)
}

/// A shot as it is drawn: its capture with the region in deck um (the
/// `--in-chip` frame applied, else the capture as given), the chips its
/// options name and the region.
struct Framed<'a> {
    capture: Cow<'a, Capture>,
    chips: Option<Selection>,
    in_chip: Option<ChipFrame>,
    region: [f64; 4],
}

/// A shot's chips and region: the chips its options name (a jobdeck's chip
/// table; a layout has none), and the region - the shot's own, else the
/// `--in-chip` placement's extent, else the extent of the chips named, else
/// `default_box` (the whole source). A region in a chip's coordinates
/// (`--in-chip NAME#K`, jobdeck::chips::ChipFrame) is placed as the deck
/// places that chip first, so the window, the tiles and the report see
/// deck um.
fn shot_region<'a>(
    named: &'a NamedCapture,
    table: Option<&ChipTable>,
    source_name: &str,
    default_box: [f64; 4],
) -> Result<Framed<'a>> {
    let mut capture = Cow::Borrowed(&named.capture);
    let mut in_chip = None;
    if let Some(token) = &named.capture.in_chip {
        let Some(table) = table else {
            return Err(Error::input(format!(
                "{}: --in-chip names a jobdeck's placement; {source_name} is a layout",
                named.name
            )));
        };
        let frame = table.frame(token)?;
        capture = Cow::Owned(named.capture.in_frame(frame.mag, frame.dx_um, frame.dy_um)?);
        in_chip = Some(frame);
    }
    let mut selection = None;
    if capture.names_chips() {
        let Some(table) = table else {
            return Err(Error::input(format!(
                "{}: --chip, --chip-off and --fit-chip name a jobdeck's chips; {source_name} is a layout",
                named.name
            )));
        };
        selection = Some(table.select(
            capture.chip.as_deref(),
            capture.chip_off.as_deref(),
            capture.fit_chip.as_deref(),
        )?);
    }
    let mut region = default_box;
    if !capture.explicit_region() {
        if let Some(frame) = &in_chip {
            // --in-chip alone frames its placement, as --fit-chip NAME#K
            region = frame.extent_um;
        } else if let Some(sel) = &selection {
            region = sel.region.ok_or_else(|| {
                Error::input(format!(
                    "{}: the chips on have nothing placed (their sources are skipped: see 'skipped' above)",
                    named.name
                ))
            })?;
        }
    }
    Ok(Framed {
        capture,
        chips: selection,
        in_chip,
        region,
    })
}

/// The window of a run of captures over a deck (jobdeck::window, user
/// 2026-10-29): every shot's tiles' regions as `run_summary` frames them -
/// the shot's own region, or the extent of the chips it names (the chip
/// table of the deck's placements, from the deck and the sources' headers:
/// `analysis`, no geometry read) - in deck um, for the open that follows.
/// None when a shot has no region of its own and names no chips (it draws
/// the whole deck), or the deck places nothing (the open says so).
pub fn deck_window(
    analysis: &Analysis,
    shots: &[NamedCapture],
    cancelled: &AtomicUsize,
) -> Result<Option<DeckWindow>> {
    if shots.iter().any(|s| !s.capture.has_region()) {
        return Ok(None);
    }
    let Some(deck_box) = analysis.model.stats.bbox_um else {
        return Ok(None);
    };
    let rows = analysis.view_rows(cancelled)?;
    let layers = rows.metadata(&analysis.model.placements)?;
    let table = ChipTable::new(&analysis.deck, &analysis.model.placements, &layers);
    let source_name = file_name(Path::new(&analysis.deck.path));
    let mut rects = Vec::new();
    for named in shots {
        check_cancelled(cancelled)?;
        let framed = shot_region(named, Some(&table), &source_name, deck_box)?;
        for tile in framed.capture.tiles(framed.region)? {
            rects.push(tile.bbox.expect("fitted tile"));
        }
    }
    DeckWindow::new(rects).map(Some)
}

/// `run`, and what the images lack.
pub fn run_summary(
    dataset: &Dataset,
    shots: &[NamedCapture],
    options: &ExportOptions,
    render: RenderOptions,
    cancelled: &Arc<AtomicUsize>,
    mut log: impl FnMut(&str),
) -> Result<Summary> {
    check_cancelled(cancelled)?;
    if shots.is_empty() || (!options.batch && shots.len() != 1) {
        return Err(Error::input("invalid capture count"));
    }
    if options.batch
        && options
            .out
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".png")
    {
        return Err(Error::input("with --batch, --out is a directory"));
    }
    let input = options.input.as_ref().map(fs::canonicalize).transpose()?;
    let mut targets = BTreeSet::new();
    let mut check_target = |path: &Path| -> Result<PathBuf> {
        let target = dataset.output_path_mode(path, options.batch)?;
        if input
            .as_ref()
            .is_some_and(|p| p == &target || fs::canonicalize(&target).ok().as_ref() == Some(p))
        {
            return Err(Error::input("output must not replace the batch input"));
        }
        // A mosaic sidecar can collide with another shot/report. Reject also
        // case-only names so the same plan is safe on macOS and Linux.
        if !targets.insert(target.to_string_lossy().to_lowercase()) {
            return Err(Error::input(
                "output/report/kept tile paths collide (including case-only names)",
            ));
        }
        Ok(target)
    };
    // a jobdeck's chip view rows, for chip on/off (jobdeck::chips)
    let table = match dataset {
        Dataset::Deck(deck) => Some(ChipTable::new(
            &deck.analysis.deck,
            &deck.analysis.model.placements,
            &deck.metadata.layers,
        )),
        Dataset::Layout(_) => None,
    };
    let mut plans = Vec::new();
    let source_name = file_name(dataset.source());
    // every shot's layers, chips and region before the worker starts: a
    // name that resolves to nothing stops the run here
    for named in shots {
        let framed = shot_region(named, table.as_ref(), &source_name, dataset.bbox_um())?;
        // the capture with its region in deck um (--in-chip's frame applied)
        let capture: &Capture = &framed.capture;
        let (selection, region) = (framed.chips, framed.region);
        // the chips on are the view's L/D pairs: the visible set
        let layers = match selection.as_ref().and_then(|s| s.keys.as_ref()) {
            Some(keys) => Some(
                keys.iter()
                    .map(|(l, d)| format!("{l}/{d}"))
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            None => capture.shot.layers.clone(),
        };
        let planned = capture.plan(region)?;
        let mut tiles = planned.shots;
        for tile in &mut tiles {
            tile.layers = layers.clone();
        }
        if dataset.is_deck() && named.capture.shot.labels {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "jobdeck labels are not supported",
            ));
        }
        let lacks = lacks(dataset, &dataset.resolve_layers(layers.as_deref())?);
        for tile in &tiles {
            if tile
                .bbox
                .unwrap()
                .iter()
                .any(|n| !(n / dataset.dbu()).is_finite())
            {
                return Err(Error::input("DBU conversion overflow"));
            }
        }
        let display = if options.batch {
            options.out.join(format!("{}.png", named.name))
        } else {
            options.out.clone()
        };
        let target = check_target(&display)?;
        // a frame over the 16 Mpx limit is drawn at the largest size of its
        // aspect within it, said here with the size asked and the region
        // (user 2026-11-04; FLOE_CAPTURE_REDUCE=off refuses it instead)
        if let Some(text) = planned.frame.over_limit_text() {
            log(&format!(
                "[{}] WARNING: {}{}: {text} - drawn at {}x{} (FLOE_CAPTURE_REDUCE=off refuses it)",
                crate::program(),
                display.display(),
                if capture.is_mosaic() { " each tile" } else { "" },
                planned.frame.width,
                planned.frame.height
            ));
        }
        let keep = if named.capture.is_mosaic() && named.capture.keep_tiles {
            sidecars(&display)?
                .iter()
                .map(|p| check_target(p))
                .collect::<Result<_>>()?
        } else {
            Vec::new()
        };
        plans.push(Plan {
            named,
            tiles,
            display,
            target,
            keep,
            lacks,
            chips: selection,
            in_chip: framed.in_chip,
            region,
            frame: planned.frame,
        });
    }
    let report = options
        .report
        .as_ref()
        .map(|p| check_target(p))
        .transpose()?;
    for path in &targets {
        if Path::new(path)
            .ancestors()
            .skip(1)
            .any(|p| targets.contains(p.to_string_lossy().as_ref()))
        {
            return Err(Error::input(
                "an output file is also another output's parent directory",
            ));
        }
    }
    if let Some(path) = &report {
        // Only the explicit batch directory will be created below. A report
        // elsewhere must already have a valid parent, before any mkdir/worker.
        if !options.batch
            || path.parent() != Some(artifact::resolve_prefix(&options.out)?.as_path())
        {
            dataset.output_path(path)?;
        }
    }
    if options.batch {
        // All destinations/values were checked with missing-parent resolution
        // before creating the explicit output directory. Never create parents
        // for an unrelated report path.
        fs::create_dir_all(&options.out)?;
    }
    for p in &plans {
        for target in std::iter::once(&p.target).chain(&p.keep) {
            dataset.output_path(target)?;
        }
    }
    if let Some(r) = &report {
        dataset.output_path(r)?;
    }
    // the viewer's fills, or the export's solid ones (Shot::archival; every
    // shot of a run has the command line's)
    let archival = shots[0].capture.shot.archival;
    let mut session = RenderSession::open(dataset, render, archival, Arc::clone(cancelled))?;
    // a capture runs as long as its frame takes (the worker's 300 s default
    // is the viewer's: an exact capture of a source of hundreds of GB is
    // drawn in bands for minutes - renderd render_banded_capture, 0.12.389);
    // FLOE_RENDER_TIMEOUT_S bounds it
    session.set_timeouts(capture_timeout(), std::time::Duration::from_secs(5));
    // the styles the frames are drawn with, for --legend
    let legend_styles = options.legend.then(|| dataset.styles(archival)).transpose()?;
    // a deck's rows are told apart by their colours as they come to paint
    // (distinct_colors): what every frame is asked, and the view's colours
    // by row, to say which rows were given another
    let distinct_base = match dataset {
        Dataset::Deck(deck) if distinct_colors() => Some(deck.distinct()?),
        _ => None,
    };
    let view_colors: BTreeMap<(u32, u32), [u8; 4]> = match &distinct_base {
        Some(_) => dataset
            .styles(archival)?
            .iter()
            .map(|style| (style.layer, style.color))
            .collect(),
        None => BTreeMap::new(),
    };
    // a level view's rows are listed by their level's head (L/0), as its
    // legend lists them
    let level_view = matches!(dataset, Dataset::Deck(deck)
        if deck.metadata.jobdeck.mode == crate::jobdeck::color::Mode::Level);
    let mut rows = Vec::new();
    let mut artifact_count = 0usize;
    for (index, plan) in plans.iter().enumerate() {
        let result = (|| -> Result<serde_json::Value> {
            let started = Instant::now();
            let capture = &plan.named.capture;
            if let (Some(sel), Some(table)) = (&plan.chips, &table) {
                log(&format!(
                    "[{}] {}: {}",
                    crate::program(),
                    plan.named.name,
                    table.describe(sel)
                ));
            }
            // --in-chip: the placement whose coordinates the region was
            // given in, and the region both ways (a mosaic's tiles in the
            // report)
            if let Some(frame) = &plan.in_chip {
                log(&format!(
                    "[{}] {}: {}",
                    crate::program(),
                    plan.named.name,
                    frame.describe((!capture.is_mosaic()).then(|| plan.tiles[0].bbox.unwrap()))
                ));
            }
            let (width, height) = (plan.frame.width, plan.frame.height);
            // the frame's scale, px per um along x and y (Fitted::ppu): the
            // report's ppu / ppu_y, and with the legend the flateyes
            // metadata's ppu and unit, as a DRC snapshot's (user 2026-11-05:
            // "메타데이터에 ppu도 추가해줘")
            let (ppu, ppu_y) = plan.frame.ppu();
            // --legend of a deck lists the rows the picture shows: each tile's
            // frame is asked which deck layers are left with a pixel of it
            // (RenderRequest::drawn), a mosaic's four together. None: not
            // asked, or a frame did not say
            let legend_rows = legend_rows();
            let ask_drawn =
                legend_styles.is_some() && dataset.is_deck() && legend_rows == LegendRows::Drawn;
            let mut drawn: Option<BTreeSet<(u32, u32)>> = ask_drawn.then(BTreeSet::new);
            // the rows' colours as the tiles' frames decide them
            // (RenderRequest::distinct): a tile is told what those before it
            // decided - a mosaic's four are one picture - and says the rows
            // it painted and in which colour
            let mut distinct = match &distinct_base {
                Some(base) => {
                    let asked = picture_distinct(dataset, base, &plan.tiles, capture.is_mosaic())?;
                    if asked.is_none() {
                        log(&format!(
                            "[{}] note: {}: too many rows are placed in the picture to tell them apart by colour - drawn in the view's colours",
                            crate::program(),
                            plan.display.display()
                        ));
                    }
                    asked
                }
                None => None,
            };
            let mut painted: BTreeMap<(u32, u32), [u8; 4]> = BTreeMap::new();
            let mut canvas = capture
                .is_mosaic()
                .then(|| Mosaic::new(width, height))
                .transpose()?;
            let mut kept = Vec::new();
            let mut png = None;
            let mut deferred = 0u64;
            let mut summary_building = 0u64;
            let mut all_complete = plan.lacks.is_empty();
            // the tiles the budget fit thinned (render::budget_fit)
            let mut fits = Vec::new();
            for (i, tile) in plan.tiles.iter().enumerate() {
                let mut request = session.shot_request(dataset, tile)?;
                if canvas.is_some() {
                    request.format = FrameFormat::Raw;
                }
                request.drawn = ask_drawn;
                request.distinct = distinct.as_ref().map(|asked| Distinct {
                    taken: taken_of(asked, &painted),
                    ..asked.clone()
                });
                let frame = session.capture(request)?;
                if distinct.is_some() {
                    match frame.distinct()? {
                        Some(rows) => painted.extend(rows),
                        // a frame that does not say decided nothing
                        None => distinct = None,
                    }
                }
                if ask_drawn {
                    let said = frame.drawn()?;
                    drawn = match (drawn.take(), said) {
                        (Some(mut rows), Some(pairs)) => {
                            rows.extend(pairs);
                            Some(rows)
                        }
                        _ => None,
                    };
                }
                if let Some(fit) = budget_fit(&frame) {
                    fits.push((i, fit));
                }
                if !dataset.is_deck()
                    || frame.labels_truncated
                    || (frame.partial && frame.deferred == 0)
                {
                    require_complete(&frame)?;
                }
                all_complete &= frame.complete();
                deferred = deferred
                    .checked_add(frame.deferred)
                    .ok_or_else(|| Error::input("deferred-page count overflow"))?;
                summary_building = summary_building
                    .checked_add(frame.summary_building)
                    .ok_or_else(|| Error::input("summary-building pass count overflow"))?;
                if let Some(canvas) = &mut canvas {
                    let rgba = &frame.bytes[16..]; // worker client validated FLOERAW1/size
                    canvas.insert(rgba, cancelled)?;
                    if !plan.keep.is_empty() {
                        kept.push(StagedArtifact::write(&plan.keep[i], cancelled, |out| {
                            mosaic::encode_png(out, width, height, rgba, cancelled)
                        })?);
                    }
                } else {
                    png = Some(frame.bytes);
                }
            }
            if index + 1 == plans.len() {
                session.close()?;
            }
            // --legend: the layers this shot draws, as the picture paints them
            // - the tiles' (the chips on, with --chip; JOBDECK-CAPTURE §17
            // item 8 before 0.12.436: every row of the levels loaded), and of
            // a deck those the picture shows: the rows the frames said a pixel
            // of theirs is left to (0.12.439), under FLOE_LEGEND_IN_VIEW=placed
            // those with a placement in the frame (0.12.438), =off every row
            let legend = legend_styles
                .as_ref()
                .map(|styles| {
                    let shown = match (legend_rows, drawn) {
                        (LegendRows::Every, _) => None,
                        (LegendRows::Drawn, Some(rows)) => Some(rows),
                        _ => placed_pairs(dataset, &plan.tiles)?,
                    };
                    // the rows in the colours the frames painted them in
                    let styles = painted_styles(styles, distinct.as_ref(), &painted);
                    crate::legend::lines(
                        dataset,
                        &styles,
                        &dataset.resolve_layers(plan.tiles[0].layers.as_deref())?,
                        shown.as_ref(),
                    )
                })
                .transpose()?;
            let legend_doc = legend.as_ref().filter(|l| !l.is_empty()).map(|l| Document {
                legend: Some(l.clone()),
                ppu: Some(ppu),
                unit: Some("um".into()),
                ..Document::default()
            });
            let mut row = serde_json::json!({"name":plan.named.name,"out":plan.display,"pixel":[width,height],
                "ppu":ppu,"ppu_y":ppu_y,
                "layers":capture.shot.layers,"depth":capture.shot.depth,"over_budget_pages":deferred,
                "summary_building":summary_building,
                "skipped_placements":plan.lacks.len(),"complete":all_complete});
            // the rows that painted and the colour each was painted in
            // (`colors`), and of them those given another colour than the
            // view's because a row painted before them had it (`recolored`)
            if distinct.is_some() {
                // by the legend's rows: a level view's level, else the row
                let listed: BTreeMap<(u32, u32), [u8; 4]> = painted
                    .iter()
                    .map(|(&row, &color)| (if level_view { (row.0, 0) } else { row }, color))
                    .collect();
                let key = |(layer, datatype): &(u32, u32)| format!("{layer}/{datatype}");
                row["colors"] = listed
                    .iter()
                    .map(|(row, color)| (key(row), crate::styles::color_text(*color).into()))
                    .collect::<serde_json::Map<_, _>>()
                    .into();
                let recolored: Vec<String> = listed
                    .iter()
                    .filter(|(row, color)| view_colors.get(row).is_some_and(|own| own != *color))
                    .map(|(row, _)| key(row))
                    .collect();
                if !recolored.is_empty() {
                    log(&format!(
                        "[{}] note: {}: {} of the {} row(s) painted drawn in another colour than the view's - a row painted before had theirs (FLOE_DECK_DISTINCT_COLORS=off keeps the view's)",
                        crate::program(),
                        plan.display.display(),
                        recolored.len(),
                        listed.len()
                    ));
                }
                row["recolored"] = serde_json::json!(recolored);
            }
            if let (Some(sel), Some(table)) = (&plan.chips, &table) {
                row["chips"] = serde_json::json!({
                    "on": table.labels(sel),
                    "chip": capture.chip, "chip_off": capture.chip_off,
                    "fit_chip": capture.fit_chip,
                    "region_um": if capture.explicit_region() { None } else { Some(plan.region) },
                    "placements": sel.placements});
            }
            if let Some(frame) = &plan.in_chip {
                // the frame (chip, chip_id, rows, n, tc, levels, mag, dx_um,
                // dy_um, extent_um) and the region drawn in the chip's um
                let mut doc = serde_json::to_value(frame)
                    .map_err(|e| Error::input(format!("report: {e}")))?;
                if capture.is_mosaic() {
                    doc["tiles_chip_um"] = TILE_NAMES
                        .iter()
                        .zip(&plan.tiles)
                        .map(|(tag, tile)| {
                            (tag.to_string(), serde_json::json!(frame.chip_rect(tile.bbox.unwrap())))
                        })
                        .collect::<serde_json::Map<_, _>>()
                        .into();
                } else {
                    doc["bbox_chip_um"] = serde_json::json!(frame.chip_rect(plan.tiles[0].bbox.unwrap()));
                }
                row["in_chip"] = doc;
            }
            let staged = if let Some(canvas) = canvas {
                let (rgba, info) = canvas.finish(capture.line, &capture.line_color, cancelled)?;
                row["pixel"] = serde_json::json!([width * 2, height * 2]);
                if let Some((aw, ah)) = plan.frame.asked {
                    row["pixel_requested"] = serde_json::json!([aw * 2, ah * 2]);
                }
                row["mosaic"] = info;
                row["tiles"] = TILE_NAMES
                    .iter()
                    .zip(&plan.tiles)
                    .map(|(tag, tile)| (tag.to_string(), serde_json::json!(tile.bbox.unwrap())))
                    .collect::<serde_json::Map<_, _>>()
                    .into();
                if let Some(doc) = &legend_doc {
                    let mut bytes = std::io::Cursor::new(Vec::new());
                    mosaic::encode_png(&mut bytes, width * 2, height * 2, &rgba, cancelled)?;
                    annotations::png::stage(&plan.target, &bytes.into_inner(), doc, cancelled)?
                } else {
                    StagedArtifact::write(&plan.target, cancelled, |out| {
                        mosaic::encode_png(out, width * 2, height * 2, &rgba, cancelled)
                    })?
                }
            } else {
                row["bbox_um"] = serde_json::json!(plan.tiles[0].bbox.unwrap());
                if let Some((aw, ah)) = plan.frame.asked {
                    row["pixel_requested"] = serde_json::json!([aw, ah]);
                }
                if let Some(doc) = &legend_doc {
                    annotations::png::stage(&plan.target, png.as_ref().unwrap(), doc, cancelled)?
                } else {
                    StagedArtifact::write(&plan.target, cancelled, |out| {
                        use std::io::Write;
                        for chunk in png.as_ref().unwrap().chunks(1024 * 1024) {
                            check_cancelled(cancelled)?;
                            out.write_all(chunk)?;
                        }
                        Ok(())
                    })?
                }
            };
            if let Some(lines) = &legend {
                row["legend"] = serde_json::json!(lines);
                if lines.is_empty() {
                    log(&format!(
                        "[{}] note: {}: no layer to list - no legend",
                        crate::program(),
                        plan.display.display()
                    ));
                }
            }
            for target in std::iter::once(&plan.target).chain(&plan.keep) {
                dataset.output_path(target)?;
            }
            for stage in std::iter::once(staged).chain(kept) {
                stage.commit(cancelled)?;
                artifact_count += 1;
            }
            row["ms"] = serde_json::json!(
                (started.elapsed().as_secs_f64() * 1000.).round_ties_even() as u64
            );
            if deferred != 0 {
                log(&format!(
                    "[{}] WARNING: {} stopped at the page budget: {deferred} page(s) not drawn",
                    crate::program(),
                    plan.display.display()
                ));
            }
            // thinned to the budget: the report's budget_fit (a mosaic's by
            // tile) and a warning - the picture has fewer small shapes than
            // the cut asked for
            if !fits.is_empty() {
                row["budget_fit"] = if capture.is_mosaic() {
                    fits.iter()
                        .map(|(i, fit)| (TILE_NAMES[*i].to_string(), fit.clone()))
                        .collect::<serde_json::Map<_, _>>()
                        .into()
                } else {
                    fits[0].1.clone()
                };
                for (i, fit) in &fits {
                    log(&format!(
                        "[{}] WARNING: {}{} thinned to fit the render budget: {}; FLOE_RUST_BUDGET_MB raises it",
                        crate::program(),
                        plan.display.display(),
                        if capture.is_mosaic() {
                            format!(" tile {}", TILE_NAMES[*i])
                        } else {
                            String::new()
                        },
                        budget_fit_brief(fit, crate::render::density_for(dataset, &capture.shot).unwrap_or(false))
                    ));
                }
            }
            let seconds = row["ms"].as_u64().unwrap_or(0) as f64 / 1000.;
            if capture.is_mosaic() {
                log(&format!(
                    "[{}] rendered {} ({}x{} mosaic of 4 x {width}x{height}, line {}) in {seconds:.2}s",
                    crate::program(),
                    plan.display.display(),
                    row["pixel"][0],
                    row["pixel"][1],
                    row["mosaic"]["line_pixels"].as_str().unwrap_or("")
                ));
            } else {
                let [x0, y0, x1, y1] = plan.tiles[0].bbox.unwrap();
                log(&format!(
                    "[{}] rendered {} ({width}x{height}, {x0:.4},{y0:.4},{x1:.4},{y1:.4} um) in {seconds:.2}s",
                    crate::program(),
                    plan.display.display()
                ));
            }
            Ok(row)
        })();
        let row = result.map_err(|e| {
            Error::new(
                e.kind,
                format!(
                    "shot {} failed; {artifact_count} artifact(s) already saved: {e}",
                    plan.named.name
                ),
            )
        })?;
        rows.push(row);
    }
    let complete = rows.iter().all(|r| r["complete"] == true);
    // what the images lack - the skipped placements of the chips some shot
    // had on - and the rest, in chips no shot had on (floe.shots run_shots)
    let shown: BTreeSet<usize> = plans.iter().flat_map(|p| p.lacks.iter().copied()).collect();
    let (lacking, off): (Vec<_>, Vec<_>) = dataset
        .skipped()
        .iter()
        .enumerate()
        .partition(|(i, _)| shown.contains(i));
    let lacking: Vec<_> = lacking.into_iter().map(|(_, r)| r).collect();
    let off: Vec<_> = off.into_iter().map(|(_, r)| r).collect();
    // a jobdeck that could not draw every placement says so in the report
    // and the log, and the caller exits non-zero (review 2026-09-09 P1-1: a
    // thinner PNG must never look complete)
    if !lacking.is_empty() {
        log(&format!(
            "[{}] WARNING: {} jobdeck placement(s) not drawn: {}",
            crate::program(),
            lacking.len(),
            brief(&lacking)
        ));
    }
    if !off.is_empty() {
        log(&format!(
            "[{}] note: {} placement(s) the load skips are in chips no shot had on: {}",
            crate::program(),
            off.len(),
            brief(&off)
        ));
    }
    let over_budget_total = rows
        .iter()
        .try_fold(0u64, |n, r| {
            n.checked_add(r["over_budget_pages"].as_u64().unwrap_or(0))
        })
        .ok_or_else(|| Error::input("report page count overflow"))?;
    // the passes left empty for want of their light source's summary
    let summary_building_total = rows
        .iter()
        .try_fold(0u64, |n, r| {
            n.checked_add(r["summary_building"].as_u64().unwrap_or(0))
        })
        .ok_or_else(|| Error::input("report pass count overflow"))?;
    if let Some(path) = report {
        let mut doc = serde_json::json!({"source":dataset.source(),"dbu":dataset.dbu(),"shots":rows,
            "cut_px":shots[0].capture.shot.detail.cut_px(),"thin":shots[0].capture.shot.thin.name(),
            // the density under the cut drawn (--density; a jobdeck draws none)
            "density":crate::render::density_for(dataset, &shots[0].capture.shot).unwrap_or(false),
            // the occupancy summary asked for (--occupancy; null = renderd's rule)
            "occupancy":shots[0].capture.shot.occupancy,
            // a layout's default layer colours (--layer-colors; a jobdeck: null)
            "layer_colors":dataset.layer_colors().map(crate::styles::LayerColors::name),
            // each image's layer legend embedded (--legend; the shots' `legend`)
            "legend":options.legend,
            // a jobdeck's rows told apart by their colours as they paint
            // (the shots' `colors` and `recolored`; a layout: false)
            "distinct_colors":distinct_base.is_some(),
            "archival":archival,"complete":complete});
        if let Dataset::Deck(deck) = dataset {
            let over_budget = doc["shots"]
                .as_array()
                .unwrap()
                .iter()
                .try_fold(0u64, |n, r| {
                    n.checked_add(r["over_budget_pages"].as_u64().unwrap())
                })
                .ok_or_else(|| Error::input("report page count overflow"))?;
            doc["jobdeck"] = serde_json::json!({"complete":complete,"skipped":lacking,"skipped_off":off,"over_budget_pages":over_budget,
                "summary_building":summary_building_total,
                "view":deck.metadata.jobdeck.mode,"levels":deck.metadata.jobdeck.levels,
                // what the capture read of the deck (jobdeck::window): the
                // CHIP blocks --chipid / --chipnumber placed, the window's
                // regions and the sources and placements it reached (null:
                // the whole load)
                "chip_blocks":deck.metadata.jobdeck.chip_blocks,
                "window":deck.metadata.jobdeck.window});
        }
        let result = (|| {
            let bytes = artifact::json_bytes(&doc, cancelled)?;
            dataset.output_path(&path)?;
            artifact::publish(&path, &bytes, cancelled)
        })();
        result.map_err(|e| Error::new(e.kind, format!("PNG was saved; report failed: {e}")))?;
        log(&format!(
            "[{}] report {} ({} shot(s){})",
            crate::program(),
            path.display(),
            shots.len(),
            if lacking.is_empty() && over_budget_total == 0 && summary_building_total == 0 {
                ""
            } else {
                ", INCOMPLETE"
            }
        ));
    }
    Ok(Summary {
        complete,
        lacking: lacking.len(),
        over_budget: over_budget_total,
        summary_building: summary_building_total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deck_legend_lists_what_the_picture_shows_unless_the_switch_says_otherwise() {
        // the default: the rows the renderer says a pixel is left to
        assert_eq!(legend_rows_of(None), LegendRows::Drawn);
        assert_eq!(legend_rows_of(Some("")), LegendRows::Drawn);
        assert_eq!(legend_rows_of(Some("on")), LegendRows::Drawn);
        assert_eq!(legend_rows_of(Some("drawn")), LegendRows::Drawn);
        // the rules of before: 0.12.438's placements in the frame, and every
        // row of the levels loaded
        assert_eq!(legend_rows_of(Some("placed")), LegendRows::Placed);
        assert_eq!(legend_rows_of(Some(" placed ")), LegendRows::Placed);
        assert_eq!(legend_rows_of(Some("off")), LegendRows::Every);
    }

    #[test]
    fn a_decks_rows_are_told_apart_by_colour_unless_the_switch_says_otherwise() {
        assert!(distinct_colors_of(None));
        assert!(distinct_colors_of(Some("")));
        assert!(distinct_colors_of(Some("on")));
        assert!(!distinct_colors_of(Some("off")));
        assert!(!distinct_colors_of(Some(" off ")));
    }

    #[test]
    fn a_picture_keeps_the_fixed_rows_it_reaches_and_is_not_asked_past_the_commands_room() {
        use floe_worker_client::RowsBy;
        let (blue, red) = ([0, 0, 255, 255], [255, 0, 0, 255]);
        let base = |by| Distinct {
            by,
            palette: vec![vec![blue, red]],
            fixed: vec![(2, 0), (5, 1), (9, 3)],
            taken: Vec::new(),
        };
        // each row its own colour: the fixed rows among those placed
        let placed = BTreeSet::from([(5, 1), (5, 2), (7, 1)]);
        let asked = distinct_for(&base(RowsBy::Pair), &placed, false).unwrap();
        assert_eq!(asked.fixed, vec![(5, 1)]);
        assert_eq!((asked.by, &asked.palette), (RowsBy::Pair, &vec![vec![blue, red]]));
        // a level's rows one colour: a level is placed by any of its rows
        let placed = BTreeSet::from([(2, 4), (7, 1)]);
        assert_eq!(distinct_for(&base(RowsBy::Layer), &placed, true).unwrap().fixed, vec![(2, 0)]);
        // a chip's rows one colour: a chip is placed under any level
        let placed = BTreeSet::from([(4, 3), (7, 2)]);
        assert_eq!(distinct_for(&base(RowsBy::Datatype), &placed, true).unwrap().fixed, vec![(9, 3)]);
        // nothing placed: nothing fixed, still asked
        assert_eq!(distinct_for(&base(RowsBy::Pair), &BTreeSet::new(), true).unwrap().fixed, vec![]);
        // a mosaic tells its later tiles a row of every group painted before
        // them: past the command's room the picture is not asked - a single
        // frame is
        let many: BTreeSet<(u32, u32)> = (0..3000).map(|n| (n, 1)).collect();
        assert!(distinct_for(&base(RowsBy::Pair), &many, true).is_none());
        assert!(distinct_for(&base(RowsBy::Pair), &many, false).is_some());
        // (levels are few however many chips are under them, and chips few
        // however many levels they are under)
        let chips: BTreeSet<(u32, u32)> = (0..3000).map(|n| (7, n)).collect();
        assert!(distinct_for(&base(RowsBy::Layer), &chips, true).is_some());
        assert!(distinct_for(&base(RowsBy::Datatype), &many, true).is_some());
        // the fixed rows alone can pass it too (a row is `L/D,`: three
        // thousand fit, six thousand do not)
        let fixed = |rows: &BTreeSet<(u32, u32)>| Distinct {
            fixed: rows.iter().copied().collect(),
            ..base(RowsBy::Pair)
        };
        assert_eq!(distinct_for(&fixed(&many), &many, false).unwrap().fixed.len(), 3000);
        let more: BTreeSet<(u32, u32)> = (0..6000).map(|n| (n, 1)).collect();
        assert!(distinct_for(&fixed(&more), &more, false).is_none());
    }

    #[test]
    fn a_legend_is_of_the_colours_the_rows_were_painted_in() {
        let (blue, red, green) = ([0, 0, 255, 255], [255, 0, 0, 255], [0, 255, 0, 255]);
        let style = |layer, color| Style {
            layer,
            color,
            fill: floe_worker_client::Fill::Speckle,
            width: 1,
        };
        use floe_worker_client::RowsBy;
        let styles = [style((1, 0), blue), style((1, 2), blue), style((11, 0), blue), style((11, 1), blue)];
        let colors = |styles: &[Style]| styles.iter().map(|s| s.color).collect::<Vec<_>>();
        let asked = |by, fixed: &[(u32, u32)]| Distinct {
            by,
            fixed: fixed.to_vec(),
            ..Distinct::default()
        };
        // nothing painted, or not asked: the styles as they are, not copied
        let level = asked(RowsBy::Layer, &[]);
        assert!(matches!(painted_styles(&styles, Some(&level), &BTreeMap::new()), Cow::Borrowed(_)));
        let painted = BTreeMap::from([((1, 2), blue), ((11, 1), red)]);
        assert!(matches!(painted_styles(&styles, None, &painted), Cow::Borrowed(_)));
        // a level's rows are one colour: level 11's chip was painted red -
        // its head, which the legend lists, is red; level 1's keep theirs
        assert_eq!(colors(&painted_styles(&styles, Some(&level), &painted)), vec![blue, blue, red, red]);
        // each row its own: the rows that painted, by their keys
        let painted = BTreeMap::from([((1, 2), green), ((11, 0), red)]);
        let own = painted_styles(&styles, Some(&asked(RowsBy::Pair, &[])), &painted);
        assert_eq!(colors(&own), vec![blue, green, red, blue]);
        // fill and width are the row's
        assert!(own.iter().all(|s| s.fill == floe_worker_client::Fill::Speckle && s.width == 1));
        // a chip one colour under every level: the row that did not paint
        // has the colour of the one that did - unless the chip is left as it
        // is (a row of it is the layer properties')
        let chips = [style((1, 3), blue), style((2, 3), blue), style((2, 4), blue)];
        let painted = BTreeMap::from([((1, 3), red)]);
        let chip = asked(RowsBy::Datatype, &[]);
        assert_eq!(colors(&painted_styles(&chips, Some(&chip), &painted)), vec![red, red, blue]);
        let left = asked(RowsBy::Datatype, &[(2, 3)]);
        assert_eq!(colors(&painted_styles(&chips, Some(&left), &painted)), vec![red, blue, blue]);
        // what a later tile is told: a row of each group that painted, the
        // groups left as they are not among them
        let painted = BTreeMap::from([((1, 3), red), ((2, 3), red), ((2, 4), green)]);
        assert_eq!(taken_of(&chip, &painted), vec![((1, 3), red), ((2, 4), green)]);
        assert_eq!(taken_of(&left, &painted), vec![((2, 4), green)]);
    }
}
