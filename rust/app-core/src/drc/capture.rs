//! Error-sheet PNGs: live design style + editable flateyes metadata. No review
//! writes, automatic indexing, Python fallback or browser download endpoint.
use super::{open_current, reviewer_tag, waive_paths, Database, ReadHit};
use crate::{
    annotations::{self, Annotation, Document},
    artifact, check_cancelled,
    dataset::Dataset,
    render::{require_complete, RenderOptions, RenderSession},
    shots::{Shot, MAX_PIXELS},
    styles::{self, LayerProps},
    svrf::Rules,
    Error, ErrorKind, Result,
};
use floe_worker_client::{FrameFormat, Layers, RenderRequest};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    ops::Range,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{atomic::AtomicUsize, Arc},
};

#[derive(Clone, Debug)]
pub struct Options {
    pub database: PathBuf,
    pub rule: String,
    pub errors: String,
    pub cap: u64,
    pub fraction: f64,
    pub reviewer: Option<String>,
    pub rules: Option<PathBuf>,
    /// None discovers SVRF isolation; Some("all") explicitly bypasses it.
    pub layers: Option<String>,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            database: PathBuf::new(),
            rule: String::new(),
            errors: "all".into(),
            cap: 200,
            fraction: 0.3,
            reviewer: None,
            rules: None,
            layers: None,
        }
    }
}
impl Options {
    pub fn validate(&self) -> Result<()> {
        if self.database.as_os_str().is_empty() || self.rule.is_empty() {
            return Err(Error::input("--drc and --drc-rule go together"));
        }
        if !self.fraction.is_finite() || self.cap == 0 {
            return Err(Error::input(
                "--drc-frac must be finite; --drc-cap must be positive",
            ));
        }
        if self.reviewer.as_ref().is_some_and(|s| s.trim().is_empty()) {
            return Err(Error::input("--floe-reviewer must not be empty"));
        }
        Ok(())
    }
}

/// Half-open local indices, not a vector proportional to the rule's size.
fn selection(spec: &str, count: u64, cap: u64) -> Result<Range<u64>> {
    if spec.is_empty() || spec == "all" {
        return Ok(0..count.min(cap));
    }
    let number = |s: &str| {
        s.trim()
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| Error::input("--drc-err needs N, A-B, or all (1-based)"))
    };
    if let Some((a, b)) = spec.split_once('-') {
        let (a, b) = (number(a)?, number(b)?);
        if b < a {
            return Err(Error::input("bad --drc-err range"));
        }
        Ok(a - 1..b.min(count).max(a - 1))
    } else {
        let n = number(spec)?;
        if n > count {
            return Err(Error::input("--drc-err exceeds the rule's error count"));
        }
        Ok(n - 1..n)
    }
}
fn region(b: [f64; 4], fraction: f64, dbu: f64) -> Result<([f64; 4], [f64; 4])> {
    let cx = (b[0] + b[2]) / 2.;
    let cy = (b[1] + b[3]) / 2.;
    let mut span = (b[2] - b[0]).max(b[3] - b[1]).max(0.) / fraction.clamp(0.02, 1.);
    if span <= 0. {
        span = 0.1;
    }
    let bbox = [
        cx - span / 2.,
        cy - span / 2.,
        cx + span / 2.,
        cy + span / 2.,
    ];
    let view = bbox.map(|v| (v / dbu).round_ties_even());
    if !bbox.iter().chain(&view).all(|v| v.is_finite())
        || bbox[0] >= bbox[2]
        || bbox[1] >= bbox[3]
        || view[0] >= view[2]
        || view[1] >= view[3]
    {
        return Err(Error::input(
            "unrepresentable DRC capture region after DBU rounding",
        ));
    }
    Ok((bbox, view))
}
fn document(
    hit: &ReadHit,
    bbox: [f64; 4],
    px: u32,
    rule: &str,
    legend: Option<Vec<String>>,
    stop: &AtomicUsize,
) -> Result<Document> {
    let e = &hit.violation;
    let ppu = f64::from(px) / (bbox[2] - bbox[0]).max(1e-9);
    let center = [(bbox[0] + bbox[2]) / 2., (bbox[1] + bbox[3]) / 2.];
    let project = |p: [f64; 2]| [(p[0] - center[0]) * ppu, (center[1] - p[1]) * ppu];
    let points = e.points_um(stop)?;
    let mut screen = Vec::with_capacity(points.len());
    for (i, p) in points.iter().enumerate() {
        if i % 1024 == 0 {
            check_cancelled(stop)?;
        }
        let p = project(*p);
        if !p.iter().all(|v| v.is_finite()) {
            return Err(Error::input("DRC pixel coordinate overflow"));
        }
        screen.push(p);
    }
    let color = if hit.status == 1 {
        "#00E676"
    } else {
        "#FF5252"
    };
    let mut doc = Document {
        ppu: Some(ppu),
        unit: Some("um".into()),
        legend,
        note: Some(format!(
            "{rule} #{}({}){}",
            hit.local + 1,
            e.number,
            if hit.status == 1 { " - waived" } else { "" }
        )),
        ..Default::default()
    };
    if e.kind == 'p' && screen.len() >= 3 {
        doc.annotations.push(Annotation::from_json(
            json!({"kind":"polygon", "points":screen,
            "color":color, "fill":(points.len() <= 256).then(|| format!("{color}80")),
            "width":2, "casing":false}),
        )?);
    } else {
        for (i, pair) in screen.chunks_exact(2).enumerate() {
            if i % 1024 == 0 {
                check_cancelled(stop)?;
            }
            if pair[0] == pair[1] {
                continue;
            }
            if doc.annotations.len() >= annotations::MAX_ANNOTATIONS {
                return Err(Error::input(
                    "DRC annotation count exceeds 100000; no partial marker exported",
                ));
            }
            doc.annotations
                .push(Annotation::from_json(json!({"kind":"line", "a":pair[0],
                "b":pair[1], "color":color, "width":2, "casing":false}))?);
        }
    }
    for segment in super::cd_segments_um(e.kind, &points, stop)? {
        let [mut a, mut b] = segment.endpoints_um.map(project);
        if a == b {
            continue;
        }
        if segment.offset {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let length = dx.hypot(dy);
            let (mut nx, mut ny) = (-dy / length, dx / length);
            if ny > 1e-12 || (ny.abs() <= 1e-12 && nx < 0.) {
                nx = -nx;
                ny = -ny;
            }
            for p in [&mut a, &mut b] {
                p[0] += nx * 14.;
                p[1] += ny * 14.;
            }
        }
        doc.annotations.push(Annotation::from_json(
            json!({"kind":"ruler", "a":a, "b":b}),
        )?);
    }
    // Fail before rendering, not after publishing an unannotated image.
    doc.serialize(None, false, stop)?;
    Ok(doc)
}
fn output(base: &Path, rule: &str, local: u64, multiple: bool) -> Result<PathBuf> {
    let default = base.as_os_str() == "view.png";
    let base = if default {
        let mut safe = String::new();
        let mut replacing = false;
        for c in rule.chars() {
            if c.is_alphanumeric() || "_.-".contains(c) {
                safe.push(c);
                replacing = false;
            } else if !replacing {
                safe.push('_');
                replacing = true;
            }
        }
        PathBuf::from(format!("{safe}.png"))
    } else {
        base.to_owned()
    };
    let name = base
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| Error::input("output requires a UTF-8 filename"))?;
    let split = if default {
        // The rule is already a stem, even if it contains only dots.
        Some(name.len() - 4)
    } else {
        name.rfind('.')
            .filter(|&i| name[..i].chars().any(|c| c != '.'))
    };
    let (stem, ext) = split.map_or((name, ".png"), |i| (&name[..i], &name[i..]));
    let name = if multiple {
        format!("{stem}_{}{ext}", local + 1)
    } else {
        format!("{stem}{ext}")
    };
    if name.len() > 255 || name.chars().any(char::is_control) {
        return Err(Error::input(
            "DRC output filename exceeds 255 bytes or contains controls",
        ));
    }
    Ok(base.with_file_name(name))
}
type LayerRow<'a> = ((u32, u32), &'a str, &'a str);
fn rows(dataset: &Dataset) -> Result<(Vec<LayerRow<'_>>, &[LayerProps])> {
    Ok(match dataset {
        Dataset::Layout(l) => (
            l.metadata
                .layers
                .iter()
                .map(|l| (l.key(), l.name.as_str(), l.color.as_str()))
                .collect(),
            &l.layer_props,
        ),
        Dataset::Deck(d) => (
            d.metadata
                .layers
                .iter()
                .map(|l| {
                    Ok((
                        (
                            u32::try_from(l.layer)
                                .map_err(|_| Error::input("invalid deck layer"))?,
                            u32::try_from(l.datatype)
                                .map_err(|_| Error::input("invalid deck datatype"))?,
                        ),
                        l.name.as_str(),
                        l.color.as_str(),
                    ))
                })
                .collect::<Result<Vec<_>>>()?,
            &d.layer_props,
        ),
    })
}
fn sidecars(o: &Options, db: &Database) -> Result<Vec<PathBuf>> {
    if let Some(path) = &o.rules {
        return Ok(vec![path.clone()]);
    }
    let mut deck = None;
    for ci in 0..db.check_count().min(50) {
        for line in db.check(ci)?.desc.lines() {
            if let Some(path) = line.strip_prefix("Rule File Pathname:") {
                if !path.trim().is_empty() {
                    deck = Some(PathBuf::from(path.trim()));
                    break;
                }
            }
        }
        if deck.is_some() {
            break;
        }
    }
    let mut paths = Vec::new();
    if let Some(deck) = deck {
        if let Some(name) = deck.file_name() {
            let adjacent = crate::cache::absolute(&o.database)?
                .parent()
                .unwrap()
                .join(name);
            paths.push(PathBuf::from(format!("{}.rules.json", adjacent.display())));
        }
        paths.push(PathBuf::from(format!("{}.rules.json", deck.display())));
    }
    paths.push(PathBuf::from(format!(
        "{}.rules.json",
        o.database.display()
    )));
    Ok(paths)
}
fn layers(
    o: &Options,
    dataset: &Dataset,
    candidates: &[PathBuf],
    stop: &AtomicUsize,
) -> Result<Layers> {
    if let Some(layers) = &o.layers {
        return dataset.resolve_layers(Some(layers));
    }
    let path = if o.rules.is_some() {
        candidates.first()
    } else {
        candidates
            .iter()
            .find(|p| fs::metadata(p).is_ok_and(|m| m.is_file()))
    };
    let Some(path) = path else {
        eprintln!("[drc] no rules.json sidecar found - rendering all layers");
        return Ok(Layers::All);
    };
    let rules = match Rules::load(path, stop) {
        Ok(rules) => rules,
        Err(e) if e.kind == ErrorKind::Cancelled => return Err(e),
        Err(e) => {
            eprintln!("[drc][warn] rules sidecar unusable ({e}) - all layers");
            return Ok(Layers::All);
        }
    };
    let Some(rule) = rules.rule(&o.rule).filter(|r| !r.source_gds.is_empty()) else {
        eprintln!(
            "[drc] rule {:?} has no svrf layer metadata - all layers",
            o.rule
        );
        return Ok(Layers::All);
    };
    let keys: Vec<_> = rows(dataset)?
        .0
        .into_iter()
        .map(|r| r.0)
        .filter(|&(l, d)| rule.includes_layer(l, d))
        .collect();
    if keys.is_empty() {
        eprintln!("[drc][warn] svrf source layers not in this design - all layers");
        Ok(Layers::All)
    } else {
        eprintln!(
            "[drc] svrf isolate {}: {}",
            o.rule,
            keys.iter()
                .map(|(l, d)| format!("{l}/{d}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        Ok(Layers::Only(keys))
    }
}
fn legend(dataset: &Dataset, layers: &Layers) -> Result<Option<Vec<String>>> {
    let Layers::Only(keys) = layers else {
        return Ok(None);
    };
    let (rows, props) = rows(dataset)?;
    let rows: BTreeMap<_, _> = rows
        .into_iter()
        .map(|(key, name, color)| (key, (name, color)))
        .collect();
    let mut fills = BTreeMap::new();
    for p in props {
        let value = p.fill.trim().to_lowercase();
        if matches!(value.as_str(), "solid" | "clear") || styles::pattern(&value).is_some() {
            fills.insert(p.layer, value);
        }
    }
    let mut lines = Vec::new();
    for key in keys {
        let Some((name, color)) = rows.get(key) else {
            continue;
        };
        let fill = fills.get(key).map_or("speckle", String::as_str);
        lines.push(annotations::legend_line(&format!(
            "box {} {fill} {} {}/{}",
            if color.is_empty() { "#808080" } else { color },
            name.trim(),
            key.0,
            key.1
        ))?);
    }
    Ok((!lines.is_empty()).then_some(lines))
}

pub fn run(
    dataset: &Dataset,
    shot: &Shot,
    out: &Path,
    options: &Options,
    render: RenderOptions,
    stop: &Arc<AtomicUsize>,
    mut log: impl FnMut(&str),
) -> Result<()> {
    options.validate()?;
    shot.validate()?;
    let px = shot.pixels.0;
    if u64::from(px) * u64::from(px) > MAX_PIXELS {
        return Err(Error::input("DRC square frame exceeds 16 Mpx"));
    }
    if !dataset.skipped().is_empty() {
        return Err(Error::new(
            ErrorKind::Incomplete,
            "DRC capture requires a complete jobdeck (skipped placements)",
        ));
    }
    let mut db = open_current(&options.database, options.reviewer.as_deref(), stop)?;
    for warning in &db.warnings {
        eprintln!("[drc] {warning}");
    }
    if db.truncated_records() != 0 {
        return Err(Error::new(
            ErrorKind::Incomplete,
            "truncated ASCII DRC input; no capture exported",
        ));
    }
    let mut matches =
        (0..db.check_count()).filter(|&i| db.check(i).is_ok_and(|c| c.name == options.rule));
    let ci = matches.next().ok_or_else(|| {
        Error::input(format!("no such rule {:?} (see drc --rules)", options.rule))
    })?;
    if matches.next().is_some() {
        eprintln!(
            "[drc][warn] duplicate rule {:?} - using the first",
            options.rule
        );
    }
    let count = db.check(ci)?.count;
    let selected = selection(&options.errors, count, options.cap)?;
    if selected.is_empty() {
        return Err(Error::input("rule/selection has no errors"));
    }
    let multiple = selected.end - selected.start > 1;
    if (options.errors.is_empty() || options.errors == "all") && count > options.cap {
        eprintln!("[drc][warn] {count} errors - rendering the first {} (--drc-cap; explicit ranges render in full)",options.cap);
    }
    let candidates = sidecars(options, &db)?;
    let visible = layers(options, dataset, &candidates, stop)?;
    let legend = legend(dataset, &visible)?;
    let mut protected = candidates;
    protected.extend([
        options.database.clone(),
        db.path().to_owned(),
        PathBuf::from(format!("{}.ice", options.database.display())),
    ]);
    protected.extend(waive_paths(
        db.path(),
        &reviewer_tag(options.reviewer.as_deref()),
    )?);
    let props = match dataset {
        Dataset::Layout(l) => l.source.clone(),
        Dataset::Deck(d) => {
            crate::jobdeck::dataset::props_source(&d.source, d.metadata.jobdeck.mode)?
        }
    };
    protected.extend([
        PathBuf::from(format!("{}.layerprops", props.display())),
        props.with_extension("layerprops"),
    ]);
    let target = |local| -> Result<(PathBuf, PathBuf)> {
        let display = output(out, &options.rule, local, multiple)?;
        let resolved = dataset.output_path(&display)?;
        let path = artifact::protected_output(&resolved, &protected, &[])?;
        if fs::metadata(&path).is_ok_and(|m| m.nlink() != 1) {
            return Err(Error::input("hardlink output is unsupported"));
        }
        Ok((display, path))
    };
    // Validate ALL destinations before spawning or replacing any artifact. Do
    // not retain a million paths/violations for an explicit large range.
    for local in selected.clone() {
        check_cancelled(stop)?;
        target(local)?;
    }
    let mut session = RenderSession::open(dataset, render, false, Arc::clone(stop))?;
    let mut saved = 0u64;
    let result = (|| {
        for local in selected {
            check_cancelled(stop)?;
            let hit = db
                .errors(ci, local, 1, stop)?
                .hits
                .into_iter()
                .next()
                .filter(|h| h.local == local)
                .ok_or_else(|| Error::input("missing DRC capture record"))?;
            let (bbox, view) = region(hit.violation.bbox_um, options.fraction, dataset.dbu())?;
            let doc = document(&hit, bbox, px, &options.rule, legend.clone(), stop)?;
            let request = RenderRequest {
                view,
                width: px,
                height: px,
                depth: shot.depth,
                cut_px: shot.detail.cut_px(),
                layers: visible.clone(),
                frames: true,
                labels: !dataset.is_deck(),
                font_px: shot.font_px,
                thin: shot.thin.effective(dataset.is_deck()),
                format: FrameFormat::Png,
                ..session.base_request()
            };
            let frame = session.capture(request)?;
            require_complete(&frame)?;
            let (display, path) = target(local)?;
            let stage = annotations::png::stage(&path, &frame.bytes, &doc, stop)?;
            db.unchanged()?;
            stage.commit(stop)?;
            saved += 1;
            log(&format!(
                "{}\t{}\t{}",
                local + 1,
                hit.violation.number,
                display.display()
            ));
        }
        session.close()?;
        Ok(())
    })();
    result.map_err(|e: Error| {
        Error::new(
            e.kind,
            format!("{e}; {saved} DRC PNG(s) already saved (not a multi-file transaction)"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranges_are_streamed_and_only_all_is_capped() {
        assert_eq!(selection("all", 1000, 200).unwrap(), 0..200);
        assert_eq!(selection("1-1000", 1000, 200).unwrap(), 0..1000);
        assert_eq!(selection("900-9999", 1000, 1).unwrap(), 899..1000);
        assert_eq!(selection("8-9", 3, 200).unwrap(), 7..7);
        for s in ["0", "-1", "2-1", "1-2-3", "한글"] {
            assert!(selection(s, 1000, 200).is_err());
        }
    }
    #[test]
    fn filenames_and_half_even_regions_match_legacy() {
        assert_eq!(
            output(Path::new("view.png"), ".", 0, false).unwrap(),
            PathBuf::from("..png")
        );
        assert_eq!(
            output(Path::new("view.png"), "..", 1, true).unwrap(),
            PathBuf::from(".._2.png")
        );
        assert_eq!(
            output(Path::new("view.png"), "RULE / 한글", 6, true).unwrap(),
            PathBuf::from("RULE_한글_7.png")
        );
        for (name, want) in [
            ("shot", "shot.png"),
            (".hidden", ".hidden.png"),
            ("foo.", "foo."),
            ("foo.jpg", "foo.jpg"),
        ] {
            assert_eq!(
                output(Path::new(name), "R", 0, false).unwrap(),
                PathBuf::from(want)
            );
        }
        assert_eq!(
            region([0.5, -1.5, 2.5, 0.5], 1., 1.).unwrap().1,
            [0., -2., 2., 0.]
        );
        assert!(region([1e308, 1e308, 1e308, 1e308], 0.3, 0.001).is_err());
        assert!(region([0.; 4], 1., 1.).is_err());
        assert_eq!(
            region([0.; 4], 1., 0.001).unwrap().0,
            [-0.05, -0.05, 0.05, 0.05]
        );
    }
}
