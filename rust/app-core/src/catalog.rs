//! Local cache metadata, not a browser-controlled filesystem endpoint. Geometry
//! stays mmap-backed in renderd; JSON frontier/minimap arrays are never retained.
use crate::{cache, check_cancelled, styles, Error, ErrorKind, Result};
use floe_worker_client::Layers;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt::Write;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SourceInfo {
    pub path: String,
    pub size: u64,
    pub mtime: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Layer {
    pub layer: u32,
    pub datatype: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub color: String,
    pub stored_shapes: u64,
}
impl Layer {
    pub fn key(&self) -> (u32, u32) {
        (self.layer, self.datatype)
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Grid {
    pub nx: u32,
    pub ny: u32,
    pub x0: i64,
    pub y0: i64,
    pub tile_w: i64,
    pub tile_h: i64,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Skeleton {
    #[serde(default)]
    pub shapes: u64,
    #[serde(default)]
    pub texts: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Metadata {
    pub version: u64,
    pub vfs: u64,
    pub src: SourceInfo,
    pub dbu: f64,
    pub top_cell: String,
    pub bbox: [i64; 4],
    pub grid: Grid,
    pub layers: Vec<Layer>,
    #[serde(default)]
    pub skeleton: Option<Skeleton>,
}
#[derive(Debug)]
pub struct Layout {
    pub source: PathBuf,
    pub directory: PathBuf,
    pub metadata: Metadata,
    pub source_stale: bool,
    pub layer_props: Vec<styles::LayerProps>,
}

pub(crate) fn regular_file(path: &Path) -> Result<File> {
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !f.metadata()?.is_file() {
        return Err(Error::input(format!(
            "not a regular file: {}",
            path.display()
        )));
    }
    Ok(f)
}
impl Layout {
    pub fn open(source: &Path, cancelled: &AtomicUsize) -> Result<Self> {
        check_cancelled(cancelled)?;
        let source = cache::absolute(source)?;
        if source
            .extension()
            .is_some_and(|s| s.eq_ignore_ascii_case("jb"))
        {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "jobdeck requires M1a-3; use existing floe2",
            ));
        }
        let fingerprint = cache::fingerprint(&source)?;
        let directory = cache::cache_path(&source)?;
        if !fs::symlink_metadata(&directory).is_ok_and(|m| m.is_dir()) {
            return Err(Error::new(
                ErrorKind::Cache,
                "no real VFS cache directory; run floe2-web index SOURCE first",
            ));
        }
        let f = regular_file(&directory.join("meta.json"))?;
        // Bounds malformed metadata, while skipping arbitrarily nested unknown
        // fields with serde's depth guard. No geometry-sized JSON Value tree.
        const MAX_META_BYTES: u64 = 256 * 1024 * 1024;
        if f.metadata()?.len() > MAX_META_BYTES {
            return Err(Error::new(ErrorKind::Cache, "metadata exceeds 256 MiB"));
        }
        let mut metadata: Metadata =
            serde_json::from_reader(BufReader::new(f.take(MAX_META_BYTES + 1))).map_err(|e| {
                Error::new(ErrorKind::Cache, format!("invalid cache metadata: {e}"))
            })?;
        if metadata.version.to_string() != env!("FLOE_CACHE_VERSION") || metadata.vfs != 1 {
            return Err(Error::new(
                ErrorKind::Cache,
                "cache version/type mismatch; re-index required",
            ));
        }
        if !metadata.dbu.is_finite()
            || metadata.dbu <= 0.
            || !metadata.dbu.recip().is_finite()
            || metadata.bbox[0] > metadata.bbox[2]
            || metadata.bbox[1] > metadata.bbox[3]
            || metadata.grid.tile_w < 0
            || metadata.grid.tile_h < 0
            || metadata.layers.len() > 65536
        {
            return Err(Error::new(
                ErrorKind::Cache,
                "invalid DBU/bbox/grid/layer count in metadata",
            ));
        }
        let mut keys = BTreeSet::new();
        if metadata.layers.iter().any(|l| !keys.insert(l.key())) {
            return Err(Error::new(ErrorKind::Cache, "duplicate layer in metadata"));
        }
        let vfs = cache::validated_vfs(&directory)?;
        if (vfs.ovm.src_size, vfs.ovm.src_mtime) != (metadata.src.size, metadata.src.mtime)
            || (metadata.dbu * vfs.ovm.unit - 1.).abs() > 1e-12
        {
            return Err(Error::new(
                ErrorKind::Cache,
                "metadata/marker identity or DBU mismatch",
            ));
        }
        drop(vfs);
        let layer_props = styles::load_props(&source)?;
        for layer in &mut metadata.layers {
            layer.color = styles::color_text(styles::layer_color(layer.layer));
            let key = layer.key();
            for prop in layer_props.iter().filter(|p| p.layer == key) {
                if let Some(color) = prop.color {
                    layer.color = styles::color_text(color);
                }
            }
        }
        check_cancelled(cancelled)?;
        let source_stale = fingerprint != (metadata.src.size, metadata.src.mtime);
        Ok(Self {
            source,
            directory,
            metadata,
            source_stale,
            layer_props,
        })
    }

    pub fn bbox_um(&self) -> [f64; 4] {
        self.metadata.bbox.map(|v| v as f64 * self.metadata.dbu)
    }
    pub fn resolve_layers(&self, spec: Option<&str>) -> Result<Layers> {
        let Some(spec) = spec.filter(|s| !s.is_empty() && *s != "all") else {
            return Ok(Layers::All);
        };
        let mut out = Vec::new();
        for token in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            if let Some((l, d)) = token.split_once('/') {
                let key = (
                    l.parse()
                        .map_err(|_| Error::input("invalid layer number"))?,
                    d.parse()
                        .map_err(|_| Error::input("invalid datatype number"))?,
                );
                if !out.contains(&key) {
                    out.push(key);
                }
            } else {
                let found: Vec<_> = self
                    .metadata
                    .layers
                    .iter()
                    .filter(|l| l.name == token || l.aliases.iter().any(|a| a == token))
                    .map(Layer::key)
                    .collect();
                if found.is_empty() {
                    return Err(Error::input(format!("unknown layer: {token}")));
                }
                for key in found {
                    if !out.contains(&key) {
                        out.push(key);
                    }
                }
            }
            if out.len() > 4096 {
                return Err(Error::input(
                    "explicit layer selection exceeds 4096 entries",
                ));
            }
        }
        Ok(if out.is_empty() {
            Layers::None
        } else {
            Layers::Only(out)
        })
    }

    pub fn summary(&self, cancelled: &AtomicUsize) -> Result<String> {
        let m = &self.metadata;
        let bb = self.bbox_um();
        let mut text = format!("source     : {} ({:.2} GB)\ntop cell   : {}\nbbox       : ({:.1}, {:.1}) - ({:.1}, {:.1}) um\ncache      : {}  ({:.1} MB, VFS v{})\n",
            m.src.path, m.src.size as f64 / 1e9, m.top_cell, bb[0], bb[1], bb[2], bb[3],
            self.directory.display(), directory_size(&self.directory, cancelled)? as f64 / 1e6, m.vfs);
        for part in ["design.ovm", "design.ovp"] {
            writeln!(
                text,
                "  {part:<11}: {:.1} MB",
                fs::metadata(self.directory.join(part))?.len() as f64 / 1e6
            )
            .unwrap();
        }
        let sk = m.skeleton.clone().unwrap_or_default();
        writeln!(
            text,
            "skeleton   : {} shapes, {} texts",
            comma(sk.shapes),
            comma(sk.texts)
        )
        .unwrap();
        writeln!(
            text,
            "{:>8}  {:<12} {:>14}",
            "layer", "name", "stored shapes"
        )
        .unwrap();
        for l in &m.layers {
            writeln!(
                text,
                "{:>5}/{:<2} {:<12} {:>14}",
                l.layer,
                l.datatype,
                l.name,
                comma(l.stored_shapes)
            )
            .unwrap();
        }
        Ok(text)
    }
}
fn directory_size(path: &Path, cancelled: &AtomicUsize) -> Result<u64> {
    let mut pending = vec![path.to_owned()];
    let mut bytes = 0u64;
    while let Some(path) = pending.pop() {
        check_cancelled(cancelled)?;
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            }
            if kind.is_file() {
                bytes = bytes
                    .checked_add(entry.metadata()?.len())
                    .ok_or_else(|| Error::input("cache size overflow"))?;
            }
        }
    }
    Ok(bytes)
}
pub(crate) fn comma(n: u64) -> String {
    let text = n.to_string();
    let mut out = String::new();
    for (i, c) in text.chars().enumerate() {
        if i > 0 && (text.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
