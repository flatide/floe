//! Composite spec is trusted application output, never user-controlled wire.
//! Missing caches/layers are explicit ledger entries, not invisible omissions.
use super::{
    geom::{Placement, Skipped},
    plan::Analysis,
};
use crate::{catalog::Layout, check_cancelled, Error, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::sync::atomic::AtomicUsize;

const SPEC_BYTES: usize = 128 * 1024 * 1024;
#[derive(Debug)]
pub struct CompositeSpec {
    pub text: String,
    pub skipped: Vec<Skipped>,
    pub placements: usize,
}
impl CompositeSpec {
    pub fn require_drawable(&self) -> Result<()> {
        if self.placements == 0 {
            Err(Error::input(
                "no placement can be drawn (empty selection or no fresh source/layer cache)",
            ))
        } else {
            Ok(())
        }
    }
}
fn hex(s: &str) -> String {
    s.as_bytes()
        .iter()
        .fold(String::with_capacity(s.len() * 2), |mut s, b| {
            write!(s, "{b:02x}").expect("String write");
            s
        })
}
fn append(out: &mut String, line: &str) -> Result<()> {
    if out
        .len()
        .checked_add(line.len())
        .and_then(|n| n.checked_add(1))
        .is_none_or(|n| n > SPEC_BYTES)
    {
        return Err(Error::input("jobdeck composite spec exceeds 128 MiB"));
    }
    out.push_str(line);
    out.push('\n');
    Ok(())
}
fn skip(p: &Placement, reason: &str, detail: String, pair: bool) -> Skipped {
    Skipped {
        chip: p.chip.clone(),
        idx: p.idx,
        tc: p.tc.clone(),
        line: -1,
        rows: 0,
        reason: reason.into(),
        detail,
        stage: "spec".into(),
        anchors: vec![[p.jx, p.jy]],
        ly: pair.then_some(p.ly),
        dt: pair.then_some(p.dt),
    }
}
pub fn compose(analysis: &Analysis, cancelled: &AtomicUsize) -> Result<CompositeSpec> {
    let rows = analysis.view_rows(cancelled)?;
    // Keep only layer keys referenced by this plan. Retaining every layer of
    // every source would multiply source_count x metadata_layers even when
    // each placement asks for a single mask layer. This table also has an
    // explicit auxiliary-memory guard independent of the encoded spec size.
    let mut wanted: BTreeMap<&str, BTreeSet<(i64, i64)>> = BTreeMap::new();
    let mut key_bytes = 0usize;
    for p in &analysis.model.placements {
        check_cancelled(cancelled)?;
        if !analysis
            .catalog
            .infos
            .get(&p.tc)
            .is_some_and(|i| i.ok() && i.indexed)
        {
            continue;
        }
        if !wanted.contains_key(p.tc.as_str()) {
            key_bytes += 128;
        }
        let keys = wanted.entry(&p.tc).or_default();
        if !keys.contains(&(p.ly, p.dt)) {
            key_bytes += 96;
        }
        if key_bytes > 64 * 1024 * 1024 {
            return Err(Error::input("deck source layer lookup exceeds 64 MiB"));
        }
        keys.insert((p.ly, p.dt));
    }
    let mut text = String::new();
    append(
        &mut text,
        "# floe2 jobdeck composite spec (docs/JOBDECK.ko.md M2)",
    )?;
    append(
        &mut text,
        &format!("deck unit={:?}", analysis.model.stats.dbu),
    )?;
    type SourceLayers = (usize, BTreeSet<(i64, i64)>);
    let mut sources: BTreeMap<&str, SourceLayers> = BTreeMap::new();
    let mut placement_text = String::new();
    let mut count = 0;
    let mut skipped = Vec::new();
    let mut seen = BTreeSet::new();
    for p in &analysis.model.placements {
        check_cancelled(cancelled)?;
        let info = analysis.catalog.infos.get(&p.tc);
        if info.is_none_or(|i| !i.ok() || !i.indexed) {
            if seen.insert((&p.chip, p.idx, &p.tc, None)) {
                skipped.push(skip(
                    p,
                    "not_indexed",
                    "no fresh <src>.floe cache (run --index)".into(),
                    false,
                ));
            }
            continue;
        }
        let info = info.expect("checked source");
        if !sources.contains_key(p.tc.as_str()) {
            let source = Layout::open(&info.path, cancelled)?;
            if source.source_stale
                || (source.metadata.dbu / info.dbu.expect("ok DBU") - 1.).abs() > 1e-12
            {
                return Err(Error::input(
                    "jobdeck source/header cache identity changed; reopen after indexing",
                ));
            }
            let mut keys = wanted.remove(p.tc.as_str()).expect("referenced source");
            let actual: BTreeSet<_> = source
                .metadata
                .layers
                .iter()
                .map(|l| (i64::from(l.layer), i64::from(l.datatype)))
                .collect();
            keys.retain(|k| actual.contains(k));
            sources.insert(&p.tc, (sources.len(), keys));
            append(
                &mut text,
                &format!(
                    "source path_hex={}",
                    hex(source.directory.to_str().expect("UTF-8 source"))
                ),
            )?;
        }
        let (source, keys) = &sources[p.tc.as_str()];
        if !keys.contains(&(p.ly, p.dt)) {
            if seen.insert((&p.chip, p.idx, &p.tc, Some((p.ly, p.dt)))) {
                skipped.push(skip(
                    p,
                    "empty_layer",
                    format!("cache has no layer {}/{}", p.ly, p.dt),
                    true,
                ));
            }
            continue;
        }
        let out = rows.output_layer(p)?;
        let scale = p.mag * info.dbu.expect("ok DBU") / analysis.model.stats.dbu;
        if !scale.is_finite() || scale <= 0. {
            return Err(Error::input("invalid deck source scale"));
        }
        append(&mut placement_text,&format!("placement source={source} layer={}/{} out={out} scale={scale:?} dx={} dy={} order={out}",p.ly,p.dt,p.ix,p.iy))?;
        count += 1;
    }
    for r in rows.rows {
        check_cancelled(cancelled)?;
        let (ly, dt) = r.native_pair()?;
        // Analysis can report names/raw tokens; the native spec only admits
        // #RRGGBB[AA]. Reject rather than inject extra fields/commands.
        let color = r
            .color
            .strip_prefix('#')
            .ok_or_else(|| Error::input("deck spec colors require #RRGGBB or #RRGGBBAA"))?;
        if !matches!(color.len(), 6 | 8) || !color.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::input("invalid deck spec color"));
        }
        append(
            &mut text,
            &format!(
                "layer out={} key={ly}/{dt} name_hex={} color={} fill=solid width=1",
                r.out,
                hex(&r.name),
                r.color
            ),
        )?;
    }
    if text
        .len()
        .checked_add(placement_text.len())
        .is_none_or(|n| n > SPEC_BYTES)
    {
        return Err(Error::input("jobdeck composite spec exceeds 128 MiB"));
    }
    text.push_str(&placement_text);
    Ok(CompositeSpec {
        text,
        skipped,
        placements: count,
    })
}
