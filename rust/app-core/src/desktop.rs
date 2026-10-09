//! What a desktop viewer (jobdeck's GTK UI, through `floe2 gtk-service`)
//! asks of a source before it draws: whether it can open as it is, the
//! meta its layer panel lists, a deck's level rows and the layer
//! properties its fills and widths come from (docs/SHARED_APP_LAYER.ko.md
//! P2: the viewer keeps its widgets, the decisions are here).
use crate::{
    cache,
    catalog::{self, Layout},
    jobdeck::{index::is_deck, parser::JobDeck, sources::SourceCatalog},
    layerprops, Error, ErrorKind, Result,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufReader, Read};
use std::path::Path;
use std::sync::atomic::AtomicUsize;

/// Whether a source opens as it is, and who holds it when not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Readiness {
    pub ready: bool,
    /// a layout's cache is current - its source unchanged since it was
    /// built (floe/cli.py `_cache_ready`, which opens such a cache at
    /// start; the others open through the viewer's index prompt). A deck:
    /// `ready`.
    pub current: bool,
    /// another run rebuilds the layout's cache whole: the refusal naming
    /// who (floe_vfs::lock); a deck's sources are judged as it opens
    pub busy: Option<String>,
}

/// A layout with a VFS cache another run is not rebuilding, or a jobdeck
/// whose drawable sources (of `levels`, None = all) all have a current
/// one - a missing or unreadable source is a skipped placement, never a
/// reason to stay closed, but one drawable source is needed
/// (floe/gui.py `_index_ready`, jobdeck/viewer.py `deck_ready`).
pub fn ready(
    source: &Path,
    levels: Option<&BTreeSet<i64>>,
    cancelled: &AtomicUsize,
) -> Result<Readiness> {
    let source = cache::absolute(source)?;
    if is_deck(&source) {
        let deck = match JobDeck::read(&source, true, cancelled) {
            Ok(deck) => deck,
            Err(e) if e.kind == ErrorKind::Cancelled => return Err(e),
            Err(_) => {
                return Ok(Readiness {
                    ready: false,
                    current: false,
                    busy: None,
                })
            }
        };
        let directory = source
            .parent()
            .ok_or_else(|| Error::input("jobdeck has no parent directory"))?;
        let mut catalog = SourceCatalog::new(directory)?;
        catalog.probe_all(deck.sources(levels), cancelled)?;
        let drawable: Vec<_> = catalog.infos.values().filter(|i| i.ok()).collect();
        let ready = !drawable.is_empty() && drawable.iter().all(|i| i.indexed);
        return Ok(Readiness {
            ready,
            current: ready,
            busy: None,
        });
    }
    let key = floe_vfs::lock::key(
        floe_vfs::lock::Kind::Vfs,
        &cache::default_cache_path(&source)?.to_string_lossy(),
    );
    if let Some(busy) = floe_vfs::lock::opening_refusal(&key) {
        return Ok(Readiness {
            ready: false,
            current: false,
            busy: Some(busy.to_string()),
        });
    }
    let directory = cache::cache_path(&source)?;
    let ready = directory.join("meta.json").is_file();
    let current = ready && cache::inspect(&source, &directory)? == cache::CacheState::Current;
    Ok(Readiness {
        ready,
        current,
        busy: None,
    })
}

/// The cache's meta.json as the viewer reads it - every key (the
/// frontier, the grid, the skeleton …) - with the layer colours this open
/// settled (the table's, then the source's layerprops; floe/cache.py
/// Cache.load).
pub fn layout_meta(layout: &Layout) -> Result<Value> {
    let f = catalog::regular_file(&layout.directory.join("meta.json"))?;
    const MAX_META_BYTES: u64 = 256 * 1024 * 1024;
    let mut meta: Value = serde_json::from_reader(BufReader::new(f.take(MAX_META_BYTES + 1)))
        .map_err(|e| Error::new(ErrorKind::Cache, format!("invalid cache metadata: {e}")))?;
    let colors: BTreeMap<(u64, u64), &str> = layout
        .metadata
        .layers
        .iter()
        .map(|l| {
            (
                (u64::from(l.layer), u64::from(l.datatype)),
                l.color.as_str(),
            )
        })
        .collect();
    if let Some(rows) = meta.get_mut("layers").and_then(Value::as_array_mut) {
        for row in rows {
            let key = (
                row.get("layer").and_then(Value::as_u64),
                row.get("datatype").and_then(Value::as_u64),
            );
            if let (Some(l), Some(d)) = key {
                if let Some(color) = colors.get(&(l, d)) {
                    row["color"] = Value::from(*color);
                }
            }
        }
    }
    Ok(meta)
}

/// The layers a viewer lists (user 2026-10-08, the field's EBEAM files:
/// Calibre listed 3.0 and 3.300 where floe listed 3.1 and 3.2 too, nothing
/// drawn on them): a layout's pair that holds no shape and no text
/// (stored_shapes 0 - only the file's LAYERNAME table names it) is left
/// out of the panel, the visible set and the layer properties; the index,
/// `info` and the renderer keep every pair. FLOE_EMPTY_LAYERS=show lists
/// every pair. Returns the pairs left out ("3/1").
pub fn list_layers(meta: &mut Value) -> Vec<String> {
    if std::env::var("FLOE_EMPTY_LAYERS").as_deref() == Ok("show") {
        return Vec::new();
    }
    let Some(rows) = meta.get_mut("layers").and_then(Value::as_array_mut) else {
        return Vec::new();
    };
    let mut left = Vec::new();
    rows.retain(|row| {
        let empty = row.get("stored_shapes").and_then(Value::as_u64) == Some(0);
        if empty {
            left.push(format!(
                "{}/{}",
                row.get("layer").and_then(Value::as_i64).unwrap_or(0),
                row.get("datatype").and_then(Value::as_i64).unwrap_or(0)
            ));
        }
        !empty
    });
    left
}

/// A rule deck's sidecar (`<deck>.rules.json`) as the viewer reads it - the
/// JSON as written, its format checked, a newer version said
/// (floe/svrf.py load_rules).
pub fn rules_sidecar(path: &Path) -> Result<(Value, Option<String>)> {
    let f = catalog::regular_file(path)?;
    const MAX_RULES_BYTES: u64 = 64 * 1024 * 1024;
    let data: Value = serde_json::from_reader(BufReader::new(f.take(MAX_RULES_BYTES + 1)))
        .map_err(|e| Error::input(format!("{}: {e}", path.display())))?;
    if data.get("format").and_then(Value::as_str) != Some("floe-svrf-rules") {
        return Err(Error::input(format!(
            "{} is not a floe-svrf-rules file",
            path.display()
        )));
    }
    let version = data.get("version").and_then(Value::as_i64).unwrap_or(0);
    let warning = (version > 1).then(|| {
        format!(
            "[floe][warn] {} is a newer rules format (v{version} > v1)",
            path.display()
        )
    });
    Ok((data, warning))
}

/// The note a viewer gives for the pairs `list_layers` left out.
pub fn unlisted_note(left: &[String]) -> Option<String> {
    if left.is_empty() {
        return None;
    }
    Some(format!(
        "[{}] {} layer{} not listed - the file's layer table names them, no shape or text is on them: {}{} (FLOE_EMPTY_LAYERS=show lists them)",
        crate::program(),
        left.len(),
        if left.len() == 1 { "" } else { "s" },
        left.iter().take(8).cloned().collect::<Vec<_>>().join(", "),
        if left.len() > 8 { ", ..." } else { "" }
    ))
}

/// The design-default layerprops rows a viewer's fills, widths and
/// visibility come from: `<props>.layerprops`, else Calibre's
/// `<stem>.layerprops`; none is an empty table (floe/cache.py
/// load_layer_props).
pub fn props_rows(props_source: &Path) -> Result<Vec<layerprops::Row>> {
    let mut full = props_source.as_os_str().to_owned();
    full.push(".layerprops");
    for path in [full.into(), props_source.with_extension("layerprops")] {
        let f = match catalog::regular_file(&path) {
            Ok(f) => f,
            Err(e) if e.kind == ErrorKind::Io => continue,
            Err(e) => return Err(e),
        };
        let mut text = String::new();
        f.take(4 * 1024 * 1024 + 1).read_to_string(&mut text)?;
        if text.len() > 4 * 1024 * 1024 {
            return Err(Error::input("layerprops exceeds 4 MiB"));
        }
        return Ok(layerprops::parse(&text)?.rows);
    }
    Ok(Vec::new())
}

/// A layerprops file's rows (Layer > load properties): the viewer applies
/// the colours, fills, widths and visibility it names.
pub fn read_props(path: &Path) -> Result<Vec<layerprops::Row>> {
    let f = catalog::regular_file(path)?;
    let mut text = String::new();
    f.take(4 * 1024 * 1024 + 1).read_to_string(&mut text)?;
    if text.len() > 4 * 1024 * 1024 {
        return Err(Error::input("layerprops exceeds 4 MiB"));
    }
    Ok(layerprops::parse(&text)?.rows)
}

/// Write `rows` as a Calibre layerprops file at `path` (through a temporary
/// name in its folder and a rename).
pub fn write_props(path: &Path, rows: &[layerprops::Row]) -> Result<()> {
    let text = layerprops::format(rows)?;
    let folder = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    };
    let name = path
        .file_name()
        .ok_or_else(|| Error::input("layerprops path names no file"))?
        .to_string_lossy();
    let tmp = folder.join(format!(".{name}.tmp-{}", std::process::id()));
    let result = std::fs::write(&tmp, text.as_bytes()).and_then(|_| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(result?)
}

/// Publish `rows` as the design default next to the source the layer
/// properties are keyed by (`<props>.layerprops`, floe/cache.py
/// save_shared_props): anyone opening the design adopts it. Its path.
pub fn publish_props(props_source: &Path, rows: &[layerprops::Row]) -> Result<std::path::PathBuf> {
    let mut path = props_source.as_os_str().to_owned();
    path.push(".layerprops");
    let path = std::path::PathBuf::from(path);
    write_props(&path, rows)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_pairs_are_left_out_and_named() {
        let mut meta = serde_json::json!({"layers": [
            {"layer": 3, "datatype": 0, "stored_shapes": 4},
            {"layer": 3, "datatype": 1, "stored_shapes": 0},
            {"layer": 5, "datatype": 0, "stored_shapes": 1},
        ]});
        let left = list_layers(&mut meta);
        assert_eq!(left, ["3/1"]);
        assert_eq!(meta["layers"].as_array().unwrap().len(), 2);
        let note = unlisted_note(&left).unwrap();
        assert!(note.ends_with("1 layer not listed - the file's layer table names them, no shape or text is on them: 3/1 (FLOE_EMPTY_LAYERS=show lists them)"), "{note}");
        assert!(unlisted_note(&[]).is_none());
    }
}
