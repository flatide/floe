//! The light cache's index (docs/DIRECT_READ_PLAN.ko.md §5.3-5.4, L4):
//! design.inv and design.seg read into memory and a design.ovm IMAGE
//! synthesized from them, so that the planner (`hier::plan_hier`), the
//! label walk, the cell tree and the renderer's page cache work on a light
//! cache as they do on a full one. Nothing of the image is written; the
//! source file stays the geometry's only copy.
//!
//! - cells: every one from the inventory (name, height, rank, the two
//!   boxes, the three layer masks, the recursive members) - the values the
//!   full build writes (prepass PrePass::from_inventory, gate G1);
//! - pages: one per (segment, layer) row of design.seg - cell, layer,
//!   bbox, records, members, max_w / max_h / max_min as the row has them,
//!   `file_off` = the segment's offset in the SOURCE, csize / usize_ the
//!   segment's bytes apportioned to the layer by its records, codec
//!   CODEC_SEGMENT (floe_ovm). Per (cell, layer) the pages form a run in
//!   segment order with a page BVH (floe_ovm::split_pbvh, as the full
//!   build's finish_layer) - so the plan's page selection, culls and
//!   budget fit read the same fields they read on a full index;
//! - placements and the instance BVH: NONE at first. A cell whose
//!   placements the planner expands is marked on the image (floe_ovm
//!   LightMarks), `fill` reads that cell's placement-flagged segments
//!   (pread + inflate + the seeded segment parser, floe_oasis
//!   doc::parse_segment), builds its BVH (floe_ovm::split_bvh, the full
//!   build's build_cell_plan) and keeps the encoded sections; `ovm` then
//!   assembles a new image with every filled cell's. The renderer loops
//!   plan -> fill -> plan until no expanded cell is unfilled (render-core
//!   Cache::filled; one round per hierarchy level at most);
//! - the hierarchy summary (the cell tree's design.ovh): from the
//!   inventory's edges in memory (`summary`), never from the image's
//!   placements, which hold the visited cells alone;
//! - texts (L6): filled with the placements. A cell's fill reads its
//!   placement- and text-flagged segments; the texts stay as the parser
//!   gives them and `ovm` writes them into the image as the full build's
//!   build_cell_texts does (per layer a run in Morton order with a text BVH
//!   past TBVH_LEAF; the strings and the point lists of Pts repetitions
//!   in the image's design.ovt bytes), so the label walk plans on a light
//!   cache what it plans on the full index. A cell of texts alone is
//!   marked for the label walk's touch only (floe_ovm LightMarks::texts):
//!   a geometry plan that passes it fills nothing.
//!
//! Refused at open: a design.seg not paired with the design.inv at hand,
//! a source that is not the one the files were made from (size, mtime),
//! and a stream with cells defined inside CBLOCKs (a segment then holds two
//! cells' shapes and the directory's rows cannot tell them apart - the
//! full index reads such a file).

use crate::hiersum;
use floe_oasis::doc::{self, Cell, CellPiece, CellTables, PlaceRec, Rep, TextRec};
use floe_oasis::geom::{box_is_empty, orient_box, rep_extent, Box4, EMPTY_BOX};
use floe_oasis::inventory::{InvCell, Inventory};
use floe_oasis::invfile::{self, Identity};
use floe_oasis::segfile::{self, SegFile, SegRow, F_CBLOCK, F_CELLS_INSIDE, F_PLACEMENTS, F_TEXTS};
use floe_oasis::segments::Snapshot;
use floe_ovm::{
    morton_key, prepare_pts, split_bvh, split_pbvh, split_tbvh, BBox, Builder, CellSink, Ovm,
    CODEC_SEGMENT, LOD_EXACT, LOD_PAGE_NONE, PBVH_LEAF, PBVH_NONE, TBVH_LEAF, TBVH_NONE, TREP_NONE,
};
use floe_tiler::Xf;
use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::Mutex;

/// the light cache's meta.json (floe-index light writes it: the viewer's
/// keys and `"cache": "light"`); its `src.path` names the source when
/// the cache is not beside it
pub const META_FILE: &str = "meta.json";

/// FLOE_LIGHT_PLACES_MB: the bytes of filled placement sections (records,
/// point pools, BVH nodes) kept across fills; past it the cells filled
/// longest ago that the fill at hand does not need are let go (they come
/// back when a plan expands them again). Default 1 GiB.
pub const PLACES_BUDGET_DEFAULT: u64 = 1 << 30;

/// A fill takes, besides the cells a plan asked for, the cells under them -
/// the plan would expand those next, one level a round, each round
/// assembling the image anew - as long as, per asked cell, their placements
/// stay within FLOE_LIGHT_PREFILL_PLACES (default 262,144; the top's
/// millions stay out) and the segments to parse for them within
/// FLOE_LIGHT_PREFILL_MB (default 4; a cell's placements come with the
/// shapes of the segments that hold them, a whole cell on a writer that
/// compresses cells whole). 0 for either fills the asked cells alone.
pub const PREFILL_PLACES_DEFAULT: u64 = 1 << 18;
pub const PREFILL_BYTES_DEFAULT: u64 = 4 << 20;

pub fn prefill_caps() -> (u64, u64) {
    let places = std::env::var("FLOE_LIGHT_PREFILL_PLACES")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(PREFILL_PLACES_DEFAULT);
    let bytes = std::env::var("FLOE_LIGHT_PREFILL_MB")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|mb| mb.saturating_mul(1 << 20))
        .unwrap_or(PREFILL_BYTES_DEFAULT);
    (places, bytes)
}

fn places_budget() -> u64 {
    std::env::var("FLOE_LIGHT_PLACES_MB")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|mb| mb.saturating_mul(1 << 20))
        .unwrap_or(PLACES_BUDGET_DEFAULT)
}

fn bbox_of(b: &Box4) -> BBox {
    if box_is_empty(b) {
        BBox::EMPTY
    } else {
        BBox {
            x0: b.0,
            y0: b.1,
            x1: b.2,
            y1: b.3,
        }
    }
}

/// one page of the synthesized directory: a (segment, layer) row
#[derive(Clone, Debug)]
pub struct LightPage {
    /// index into `SegFile::segs`
    pub seg: u32,
    pub layer_idx: u32,
    /// position in the (cell, layer) run, segment order
    pub seq: u32,
    pub bbox: BBox,
    /// the segment's file bytes / record bytes apportioned to the layer
    pub csize: u32,
    pub usize_: u32,
    pub records: u32,
    pub members: u64,
    pub max_w: u64,
    pub max_h: u64,
    pub max_min: u64,
}

/// a filled cell's placement sections, cell-local indices (CellSink's),
/// and its texts as the parser gives them (encoded into each image)
struct CellFill {
    places: Vec<u8>,
    n_places: u64,
    pool: Vec<u8>,
    bvh: Vec<u8>,
    n_bvh: u32,
    /// the cell's TEXT records in file order (the full build's cell.texts)
    texts: Vec<TextRec>,
    bytes: u64,
    /// fill order (the budget lets the lowest go first)
    serial: u64,
}

/// whether the image needs a fill for a cell at all: the inventory counts
/// placements or texts of it
fn needs_fill(cell: &InvCell) -> bool {
    cell.placements > 0 || cell.texts > 0
}

/// the record bytes a fill of `ci` reads: its placement- and text-flagged
/// segments (a CBLOCK's inflated bytes)
fn fill_bytes(seg: &SegFile, ci: usize) -> u64 {
    let (first, count) = seg.cells[ci];
    (first..first + count)
        .map(|si| &seg.segs[si as usize])
        .filter(|s| s.flags & (F_PLACEMENTS | F_TEXTS) != 0)
        .map(|s| if s.inflated == 0 { s.bytes } else { s.inflated })
        .sum()
}

#[derive(Default)]
struct Fills {
    cells: Vec<Option<CellFill>>,
    bytes: u64,
    serial: u64,
    /// cells filled over the index's life, and let go by the budget
    filled: u64,
    evicted: u64,
}

/// What one `fill` did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FillReport {
    /// cells newly filled by this call (already filled ones skipped)
    pub cells: usize,
    pub placements: u64,
    pub texts: u64,
    pub segments: usize,
    pub evicted: usize,
    pub bytes: u64,
}

/// Open source files, shared by every light index of the process and
/// bounded (`SOURCE_FILES`): a jobdeck opens one light cache per source -
/// hundreds in the field - and a file descriptor each would run a tight
/// open-file limit out (the full index keeps none: its files are mapped,
/// its page file opened per read). A read takes the file from the pool,
/// opening it when it is not there and letting the least recently used go
/// past the cap; a reader in the middle of a pread keeps its own Arc.
struct SourceFiles {
    files: Vec<(String, std::sync::Arc<std::fs::File>, u64)>,
    tick: u64,
}

/// open source files kept at most (the full index's budget under a tight
/// limit leaves this much room beside the locks)
const SOURCE_FILES_CAP: usize = 8;

static SOURCE_FILES: Mutex<SourceFiles> = Mutex::new(SourceFiles {
    files: Vec::new(),
    tick: 0,
});

/// the source at `path`, open for pread, from the pool
fn source_file(path: &str) -> Result<std::sync::Arc<std::fs::File>, String> {
    let mut pool = SOURCE_FILES.lock().unwrap_or_else(|p| p.into_inner());
    pool.tick += 1;
    let tick = pool.tick;
    if let Some(slot) = pool.files.iter_mut().find(|(p, _, _)| p == path) {
        slot.2 = tick;
        return Ok(std::sync::Arc::clone(&slot.1));
    }
    let file = std::fs::File::open(path).map_err(|e| format!("open {}: {}", path, e))?;
    let file = std::sync::Arc::new(file);
    while pool.files.len() >= SOURCE_FILES_CAP {
        let oldest = pool
            .files
            .iter()
            .enumerate()
            .min_by_key(|(_, (_, _, t))| *t)
            .map(|(i, _)| i)
            .unwrap_or(0);
        pool.files.remove(oldest);
    }
    pool.files.push((path.to_string(), std::sync::Arc::clone(&file), tick));
    Ok(file)
}

/// The light index of one cache directory.
pub struct LightIndex {
    pub dir: String,
    /// the source file's path (meta.json's `src.path`, else the sibling
    /// the directory's name says)
    pub source: String,
    pub identity: Identity,
    pub inv: Inventory,
    pub seg: SegFile,
    cell_index: HashMap<String, usize>,
    /// (layer, datatype) -> index into the inventory's layer_order (the
    /// text builder's lidx)
    layer_index: HashMap<(u32, u32), u32>,
    /// in page-id order
    pages: Vec<LightPage>,
    /// per cell: (first page, count)
    cell_pages: Vec<(u32, u32)>,
    /// per cell: its page BVH and page runs (local indices; places and
    /// BVH empty) - the static part of its sink
    cell_static: Vec<CellSink>,
    fills: Mutex<Fills>,
    budget: u64,
}

impl std::fmt::Debug for LightIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "LightIndex({}: {} cells, {} segments, {} pages)",
            self.dir,
            self.inv.cells.len(),
            self.seg.segs.len(),
            self.pages.len()
        )
    }
}

/// The source `dir` was made from: meta.json's `src.path` when the file
/// says one, else the sibling `.NAME.ice` -> `NAME`.
pub fn source_of(dir: &str) -> Option<String> {
    if let Ok(meta) = std::fs::read_to_string(Path::new(dir).join(META_FILE)) {
        if let Some(path) = meta_src_path(&meta) {
            return Some(path);
        }
    }
    let p = Path::new(dir);
    let name = p.file_name()?.to_str()?;
    let base = name.strip_prefix('.')?.strip_suffix(".ice")?;
    if base.is_empty() {
        return None;
    }
    let parent = p.parent().filter(|d| !d.as_os_str().is_empty());
    Some(match parent {
        Some(d) => d.join(base).to_string_lossy().into_owned(),
        None => base.to_string(),
    })
}

/// `"src": {"path": "..."}` out of meta.json (the indexer's own format,
/// escapes as cli jesc writes them)
fn meta_src_path(meta: &str) -> Option<String> {
    let at = meta.find("\"src\"")?;
    let rest = &meta[at..];
    let at = rest.find("\"path\"")?;
    let rest = &rest[at + 6..];
    let q = rest.find('"')?;
    let mut chars = rest[q + 1..].chars();
    let mut out = String::new();
    loop {
        match chars.next()? {
            '"' => break,
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                c => out.push(c),
            },
            c => out.push(c),
        }
    }
    (!out.is_empty()).then_some(out)
}

/// (size, mtime seconds) of a file as the indexer records them
/// (cli vfs.rs source_identity)
pub fn file_identity(path: &str) -> std::io::Result<Identity> {
    let meta = std::fs::metadata(path)?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(Identity {
        src_size: meta.len(),
        src_mtime: mtime,
    })
}

/// What a light cache's headers say (`check`): the files are there, paired
/// and whole by their lengths - without reading their bodies or the source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightFiles {
    pub identity: Identity,
    pub unit: f64,
    pub n_cells: u32,
    pub n_layers: u32,
    pub n_segments: u32,
    /// design.ovo is there (the wide views' summary)
    pub occupancy: bool,
}

/// A light cache's files checked by their headers (app-core's cache probe,
/// which must not read a 64 MB inventory per open): design.inv and
/// design.seg present, read by this build, paired (design.seg's inv_crc is
/// design.inv's body CRC, the same identity, the same counts) and whole
/// (each body as long as its header says).
pub fn check(dir: &str) -> Result<LightFiles, String> {
    use std::io::Read;
    let head = |name: &str, len: usize| -> Result<(Vec<u8>, u64), String> {
        let path = Path::new(dir).join(name);
        let mut f = std::fs::File::open(&path).map_err(|e| format!("{}: {}", path.display(), e))?;
        let size = f.metadata().map_err(|e| format!("{}: {}", path.display(), e))?.len();
        let mut bytes = vec![0u8; len];
        f.read_exact(&mut bytes)
            .map_err(|e| format!("{}: shorter than its header ({})", path.display(), e))?;
        Ok((bytes, size))
    };
    let (ib, isize_) = head(invfile::FILE_NAME, invfile::HEADER_LEN)?;
    let ih = invfile::peek_header(&ib).map_err(|e| format!("{}: {}", dir, e))?;
    if ih.body_len + invfile::HEADER_LEN as u64 != isize_ {
        return Err(format!(
            "{}/{}: {} bytes, {} by its header (truncated or written over)",
            dir,
            invfile::FILE_NAME,
            isize_,
            ih.body_len + invfile::HEADER_LEN as u64
        ));
    }
    let (sb, ssize) = head(segfile::FILE_NAME, segfile::HEADER_LEN)?;
    let sh = segfile::peek_header(&sb).map_err(|e| format!("{}: {}", dir, e))?;
    if sh.body_len + segfile::HEADER_LEN as u64 != ssize {
        return Err(format!(
            "{}/{}: {} bytes, {} by its header (truncated or written over)",
            dir,
            segfile::FILE_NAME,
            ssize,
            sh.body_len + segfile::HEADER_LEN as u64
        ));
    }
    if sh.identity != ih.identity || sh.inv_crc != ih.body_crc {
        return Err(format!(
            "{}: design.seg is not paired with design.inv (another inventory or source); rebuild the light cache",
            dir
        ));
    }
    if sh.n_cells as usize != ih.n_cells || sh.n_layers as usize != ih.n_layers {
        return Err(format!(
            "{}: design.seg has {} cells and {} layers, design.inv {} and {}",
            dir, sh.n_cells, sh.n_layers, ih.n_cells, ih.n_layers
        ));
    }
    Ok(LightFiles {
        identity: ih.identity,
        unit: ih.unit,
        n_cells: sh.n_cells,
        n_layers: sh.n_layers,
        n_segments: sh.n_segments,
        occupancy: std::fs::symlink_metadata(Path::new(dir).join("design.ovo")).is_ok_and(|m| m.is_file()),
    })
}

impl LightIndex {
    /// Whether `dir` holds a light cache's files (design.inv; the renderer
    /// opens a directory without design.ovm this way).
    pub fn present(dir: &str) -> bool {
        std::fs::symlink_metadata(Path::new(dir).join(invfile::FILE_NAME)).is_ok()
    }

    pub fn open(dir: &str) -> Result<LightIndex, String> {
        let inv_path = Path::new(dir).join(invfile::FILE_NAME);
        let seg_path = Path::new(dir).join(segfile::FILE_NAME);
        let inv_bytes =
            std::fs::read(&inv_path).map_err(|e| format!("{}: {}", inv_path.display(), e))?;
        let (identity, inv) = invfile::decode(&inv_bytes)
            .map_err(|e| format!("{}: {}", inv_path.display(), e))?;
        let (sid, seg) =
            segfile::read(&seg_path).map_err(|e| format!("{}: {}", seg_path.display(), e))?;
        if sid != identity {
            return Err(format!(
                "light index: {} was made from another source than {} ({}/{} vs {}/{})",
                seg_path.display(),
                inv_path.display(),
                sid.src_size,
                sid.src_mtime,
                identity.src_size,
                identity.src_mtime
            ));
        }
        let crc = segfile::inv_crc_of(&inv_bytes).unwrap_or(0);
        if seg.inv_crc != crc {
            return Err(format!(
                "light index: {} is not paired with {} (inventory checksum {:08x}, the directory says {:08x}); rebuild the light cache",
                seg_path.display(),
                inv_path.display(),
                crc,
                seg.inv_crc
            ));
        }
        if seg.n_cells as usize != inv.cells.len() || seg.n_layers as usize != inv.layer_order.len() {
            return Err(format!(
                "light index: {} has {} cells and {} layers, the inventory {} and {}",
                seg_path.display(),
                seg.n_cells,
                seg.n_layers,
                inv.cells.len(),
                inv.layer_order.len()
            ));
        }
        let source = source_of(dir).ok_or_else(|| {
            format!(
                "light index: {} does not say which source it was made from (no meta.json src.path, and the name is not .<source>.ice)",
                dir
            )
        })?;
        let now = file_identity(&source).map_err(|e| format!("light index: source {}: {}", source, e))?;
        if now != identity {
            return Err(format!(
                "light index: {} is not the source {} was made from (size {} mtime {} now, {} / {} then); rebuild the light cache",
                source, dir, now.src_size, now.src_mtime, identity.src_size, identity.src_mtime
            ));
        }
        // (opened through the pool: a descriptor is not kept per index)
        drop(source_file(&source)?);
        let inside = seg.segs.iter().filter(|s| s.flags & F_CELLS_INSIDE != 0).count();
        let cblock_cells = inv.cells.iter().filter(|c| c.cblock_defs > 0).count();
        if inside > 0 || cblock_cells > 0 {
            return Err(format!(
                "light index: {} cell(s) of {} are defined inside CBLOCKs ({} segment(s) hold another cell's records); the light cache cannot read them - use the full index",
                cblock_cells, source, inside
            ));
        }
        let n = inv.cells.len();
        // ---- the page directory: per cell, per layer in layer order, the
        // cell's segments with a row of that layer, in segment order; the
        // run's page BVH when it is longer than a leaf
        let mut pages: Vec<LightPage> = Vec::with_capacity(seg.layers.len());
        let mut cell_pages: Vec<(u32, u32)> = Vec::with_capacity(n);
        let mut cell_static: Vec<CellSink> = Vec::with_capacity(n);
        for ci in 0..n {
            let (first, count) = seg.cells[ci];
            let page_base = pages.len();
            let mut sink = CellSink::default();
            let mut per_layer: BTreeMap<u32, Vec<(u32, usize)>> = BTreeMap::new();
            for si in first..first + count {
                let s = &seg.segs[si as usize];
                for lr in s.layers_first..s.layers_first + s.layers_count {
                    per_layer
                        .entry(seg.layers[lr as usize].layer_idx)
                        .or_default()
                        .push((si, lr as usize));
                }
            }
            for (li, run) in per_layer {
                let run_lo = (pages.len() - page_base) as u32;
                let mut order: Vec<usize> = (0..run.len()).collect();
                let mut root = PBVH_NONE;
                if run.len() > PBVH_LEAF {
                    let mut items: Vec<(BBox, u64, u64, usize)> = run
                        .iter()
                        .enumerate()
                        .map(|(k, &(_, lr))| {
                            let row = &seg.layers[lr];
                            (
                                bbox_of(&row.bbox),
                                row.max_w.max(0) as u64,
                                row.max_h.max(0) as u64,
                                k,
                            )
                        })
                        .collect();
                    let mut nodes: Vec<(BBox, u32, u16, bool, u64, u64)> =
                        vec![(BBox::EMPTY, 0, 0, false, 0, 0)];
                    split_pbvh(&mut nodes, &mut items, 0, 0);
                    let root_local = sink.n_pbvh;
                    for (bb, f, c, leaf, mw, mh) in nodes {
                        let first = if leaf { run_lo + f } else { root_local + f };
                        sink.pbvh_node(&bb, first, c, leaf, mw, mh);
                    }
                    root = root_local;
                    order = items.iter().map(|&(.., k)| k).collect();
                }
                for k in order {
                    let (si, lr) = run[k];
                    let s = &seg.segs[si as usize];
                    let row = &seg.layers[lr];
                    let (csize, usize_) = apportion(s, row.records)?;
                    pages.push(LightPage {
                        seg: si,
                        layer_idx: li,
                        seq: k as u32,
                        bbox: bbox_of(&row.bbox),
                        csize,
                        usize_,
                        records: u32::try_from(row.records).map_err(|_| {
                            format!("light index: a segment at {} holds {} records of one layer (more than a page can)", s.offset, row.records)
                        })?,
                        members: row.members,
                        max_w: row.max_w.max(0) as u64,
                        max_h: row.max_h.max(0) as u64,
                        max_min: row.max_min.max(0) as u64,
                    });
                }
                sink.prange(li, run_lo, run.len() as u32, root);
            }
            let count = u32::try_from(pages.len() - page_base)
                .map_err(|_| "light index: more pages in one cell than the index can hold".to_string())?;
            cell_pages.push((page_base as u32, count));
            cell_static.push(sink);
        }
        u32::try_from(pages.len())
            .map_err(|_| "light index: more pages than the index can hold".to_string())?;
        let cell_index = inv.cell_index();
        let layer_index = inv
            .layer_order
            .iter()
            .enumerate()
            .map(|(i, &key)| (key, i as u32))
            .collect();
        Ok(LightIndex {
            dir: dir.to_string(),
            source,
            identity,
            inv,
            seg,
            cell_index,
            layer_index,
            pages,
            cell_pages,
            cell_static,
            fills: Mutex::new(Fills {
                cells: (0..n).map(|_| None).collect(),
                ..Fills::default()
            }),
            budget: places_budget(),
        })
    }

    pub fn n_pages(&self) -> u32 {
        self.pages.len() as u32
    }

    pub fn page(&self, page_id: u32) -> Option<&LightPage> {
        self.pages.get(page_id as usize)
    }

    pub fn segment(&self, si: u32) -> &SegRow {
        &self.seg.segs[si as usize]
    }

    /// the (layer, datatype) pair of a layer index
    pub fn layer_key(&self, layer_idx: u32) -> (u32, u32) {
        self.inv.layer_order[layer_idx as usize]
    }

    pub fn unit(&self) -> f64 {
        self.inv.unit()
    }

    /// the placement sections' bytes resident, the cells filled over the
    /// index's life and the cells the budget let go
    pub fn fill_stats(&self) -> (u64, u64, u64) {
        let f = self.fills.lock().unwrap_or_else(|p| p.into_inner());
        (f.bytes, f.filled, f.evicted)
    }

    // ------------------------------------------------------ the source

    fn read_at(&self, offset: u64, bytes: u64) -> Result<Vec<u8>, String> {
        let n = usize::try_from(bytes).map_err(|_| "segment too large".to_string())?;
        let mut buf = vec![0u8; n];
        source_file(&self.source)?
            .read_exact_at(&mut buf, offset)
            .map_err(|e| format!("{}: read {} bytes at {}: {}", self.source, bytes, offset, e))?;
        Ok(buf)
    }

    /// a segment's file bytes (a CBLOCK: the record; a span: the records)
    pub fn segment_bytes(&self, si: u32) -> Result<Vec<u8>, String> {
        let s = &self.seg.segs[si as usize];
        self.read_at(s.offset, s.bytes)
    }

    /// a segment's record stream: a CBLOCK's inflated body, a span's bytes
    fn stream_of(&self, si: u32) -> Result<Vec<u8>, String> {
        let s = &self.seg.segs[si as usize];
        let raw = self.read_at(s.offset, s.bytes)?;
        if s.flags & F_CBLOCK != 0 {
            floe_oasis::inflate_cblock_record(&raw, s.offset as usize).map_err(|e| e.to_string())
        } else {
            Ok(raw)
        }
    }

    fn snapshot_of(&self, s: &SegRow) -> Result<Snapshot, String> {
        let (a, b) = (
            s.snapshot_off as usize,
            (s.snapshot_off + s.snapshot_len) as usize,
        );
        segfile::decode_snapshot(&self.seg.snaps[a..b], s.offset as usize).map_err(|e| e.to_string())
    }

    /// The records of segment `si` parsed from `raw` (its file bytes,
    /// `segment_bytes`) with its snapshot: the pieces of the cells it holds
    /// - one, the segment's cell, in a stream the index accepted.
    pub fn parse_segment(&self, si: u32, raw: &[u8]) -> Result<Vec<CellPiece>, String> {
        let s = &self.seg.segs[si as usize];
        if raw.len() as u64 != s.bytes {
            return Err(format!(
                "light index: segment at {} is {} bytes, {} read",
                s.offset,
                s.bytes,
                raw.len()
            ));
        }
        let snap = self.snapshot_of(s)?;
        let fields = snap
            .seed_fields(|seg, off, len| {
                let stream = self.stream_of(seg).map_err(|e| {
                    floe_oasis::OasisError::Io(std::io::Error::other(e))
                })?;
                let (a, b) = (off as usize, off as usize + len as usize);
                if b > stream.len() {
                    return Err(floe_oasis::OasisError::Io(std::io::Error::other(format!(
                        "snapshot refers past segment {}'s stream",
                        seg
                    ))));
                }
                Ok(stream[a..b].to_vec())
            })
            .map_err(|e| format!("light index: segment at {}: seed: {}", s.offset, e))?;
        let tables = CellTables::new(&self.inv.cellnames, &self.inv.textstrings, &self.cell_index);
        let name = &self.inv.cells[s.cell as usize].name;
        doc::parse_segment(raw, s.offset, name, &snap, &fields, &tables)
            .map_err(|e| format!("light index: segment at {} ({}): {}", s.offset, name, e))
    }

    /// `parse_segment` of the segment read from the source, the piece of the
    /// segment's own cell (the others, if any, are a stream the index
    /// refuses).
    pub fn segment_cell(&self, si: u32) -> Result<Cell, String> {
        let raw = self.segment_bytes(si)?;
        let s = &self.seg.segs[si as usize];
        let name = self.inv.cells[s.cell as usize].name.clone();
        let mut out = Cell {
            name,
            ..Default::default()
        };
        for p in self.parse_segment(si, &raw)? {
            if p.index != s.cell as usize {
                return Err(format!(
                    "light index: segment at {} holds records of cell {} ({}) besides its own",
                    s.offset, p.index, p.cell.name
                ));
            }
            out.rects.extend(p.cell.rects);
            out.polys.extend(p.cell.polys);
            out.paths.extend(p.cell.paths);
            out.places.extend(p.cell.places);
            out.texts.extend(p.cell.texts);
        }
        Ok(out)
    }

    // ------------------------------------------------------- the fill

    /// Reads the placements of `cells` (those the inventory gives
    /// placements and the index has not filled yet), builds their instance
    /// BVHs and keeps the sections for the next `ovm`. Past the places
    /// budget, filled cells this call does not need go, oldest first.
    pub fn fill(&self, cells: &[u32]) -> Result<FillReport, String> {
        let mut todo: Vec<u32> = {
            let f = self.fills.lock().unwrap_or_else(|p| p.into_inner());
            cells
                .iter()
                .copied()
                .filter(|&ci| {
                    (ci as usize) < self.inv.cells.len()
                        && needs_fill(&self.inv.cells[ci as usize])
                        && f.cells[ci as usize].is_none()
                })
                .collect()
        };
        todo.sort_unstable();
        todo.dedup();
        let mut report = FillReport::default();
        if todo.is_empty() {
            return Ok(report);
        }
        let jobs = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
        let (mut pieces_of, segments) = self.pieces_of(&todo, jobs, true)?;
        report.segments = segments;
        let mut built: Vec<(u32, CellFill)> = Vec::with_capacity(todo.len());
        for &ci in &todo {
            let (places, texts) = pieces_of.remove(&ci).unwrap_or_default();
            report.placements += places.len() as u64;
            report.texts += texts.len() as u64;
            built.push((ci, self.encode_places(&places, texts)));
        }
        let mut f = self.fills.lock().unwrap_or_else(|p| p.into_inner());
        let adding: u64 = built.iter().map(|(_, c)| c.bytes).sum();
        // the budget: let the oldest fills go, never one asked for now
        if f.bytes.saturating_add(adding) > self.budget {
            let mut old: Vec<(u64, usize)> = f
                .cells
                .iter()
                .enumerate()
                .filter_map(|(ci, c)| c.as_ref().map(|c| (c.serial, ci)))
                .filter(|&(_, ci)| todo.binary_search(&(ci as u32)).is_err())
                .collect();
            old.sort_unstable();
            for (_, ci) in old {
                if f.bytes.saturating_add(adding) <= self.budget {
                    break;
                }
                if let Some(c) = f.cells[ci].take() {
                    f.bytes -= c.bytes;
                    f.evicted += 1;
                    report.evicted += 1;
                }
            }
        }
        for (ci, mut c) in built {
            if f.cells[ci as usize].is_some() {
                // (another thread filled it meanwhile: keep theirs)
                continue;
            }
            f.serial += 1;
            c.serial = f.serial;
            f.bytes += c.bytes;
            f.filled += 1;
            report.cells += 1;
            report.bytes += c.bytes;
            f.cells[ci as usize] = Some(c);
        }
        Ok(report)
    }

    /// The cells a fill for `touched` takes: those, and under each the
    /// cells whose placements a later round would ask for anyway (the
    /// inventory's children, depth first), within FLOE_LIGHT_PREFILL_PLACES
    /// placements per touched cell - a cell past what is left of it, and
    /// everything under it, waits for its own round.
    pub fn fill_set(&self, touched: &[u32]) -> Vec<u32> {
        let (places, bytes) = prefill_caps();
        self.fill_set_within(touched, places, bytes)
    }

    /// `fill_set` with the caps given (0: the asked cells alone).
    pub fn fill_set_within(&self, touched: &[u32], places_cap: u64, bytes_cap: u64) -> Vec<u32> {
        let inv = &self.inv;
        let n = inv.cells.len();
        let mut out: Vec<u32> = Vec::new();
        let mut seen = vec![false; n];
        let f = self.fills.lock().unwrap_or_else(|p| p.into_inner());
        for &c in touched {
            if c as usize >= n || seen[c as usize] {
                continue;
            }
            seen[c as usize] = true;
            out.push(c);
            if places_cap == 0 || bytes_cap == 0 {
                continue;
            }
            let (mut left_places, mut left_bytes) = (places_cap, bytes_cap);
            let mut stack: Vec<usize> = inv.cells[c as usize].edges.iter().map(|e| e.child).collect();
            while let Some(k) = stack.pop() {
                if k >= n || seen[k] {
                    continue;
                }
                seen[k] = true;
                let cell = &inv.cells[k];
                if !needs_fill(cell) {
                    continue;
                }
                if f.cells[k].is_none() {
                    let b = fill_bytes(&self.seg, k);
                    if cell.placements > left_places || b > left_bytes {
                        seen[k] = false;
                        continue;
                    }
                    left_places -= cell.placements;
                    left_bytes -= b;
                    out.push(k as u32);
                }
                stack.extend(cell.edges.iter().map(|e| e.child));
            }
        }
        out
    }

    /// The placements of `cells` from their placement-flagged segments
    /// (pread + inflate + the seeded parse, `jobs` threads), each cell's in
    /// file order and as many as the inventory counts; and the segments
    /// read. A cell not in the list, or without placements, is absent.
    pub fn placements_of(&self, cells: &[u32], jobs: usize) -> Result<(HashMap<u32, Vec<PlaceRec>>, usize), String> {
        let (pieces, segments) = self.pieces_of(cells, jobs, false)?;
        Ok((pieces.into_iter().map(|(ci, (places, _))| (ci, places)).collect(), segments))
    }

    /// `placements_of`, and with `texts` the cells' TEXT records from their
    /// text-flagged segments as well (a fill reads both at once): per cell
    /// (placements, texts), each in file order and as many as the inventory
    /// counts. A cell not in the list, or with nothing to read, is absent.
    fn pieces_of(
        &self,
        cells: &[u32],
        jobs: usize,
        texts: bool,
    ) -> Result<(HashMap<u32, (Vec<PlaceRec>, Vec<TextRec>)>, usize), String> {
        let flags = if texts { F_PLACEMENTS | F_TEXTS } else { F_PLACEMENTS };
        let wanted = |cell: &InvCell| cell.placements > 0 || (texts && cell.texts > 0);
        let tasks: Vec<(u32, u32)> = cells
            .iter()
            .filter(|&&ci| (ci as usize) < self.inv.cells.len() && wanted(&self.inv.cells[ci as usize]))
            .flat_map(|&ci| {
                let (first, count) = self.seg.cells[ci as usize];
                (first..first + count)
                    .filter(move |&si| self.seg.segs[si as usize].flags & flags != 0)
                    .map(move |si| (ci, si))
            })
            .collect();
        type Got = Result<(Vec<PlaceRec>, Vec<TextRec>), String>;
        let slots: Vec<Mutex<Option<Got>>> = (0..tasks.len()).map(|_| Mutex::new(None)).collect();
        let jobs = jobs.min(tasks.len()).max(1);
        let next = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..jobs {
                scope.spawn(|| loop {
                    let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(&(_, si)) = tasks.get(k) else { break };
                    let got = self
                        .segment_cell(si)
                        .map(|c| (c.places, if texts { c.texts } else { Vec::new() }));
                    *slots[k].lock().unwrap_or_else(|p| p.into_inner()) = Some(got);
                });
            }
        });
        let mut pieces_of: HashMap<u32, (Vec<PlaceRec>, Vec<TextRec>)> = HashMap::new();
        for (k, &(ci, _)) in tasks.iter().enumerate() {
            let (places, recs) = slots[k]
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .take()
                .unwrap_or_else(|| Err("placement task without a result".to_string()))?;
            let entry = pieces_of.entry(ci).or_default();
            entry.0.extend(places);
            entry.1.extend(recs);
        }
        for &ci in cells {
            let Some(cell) = self.inv.cells.get(ci as usize) else { continue };
            if !wanted(cell) {
                continue;
            }
            let (places, recs) = pieces_of.get(&ci).map_or((0, 0), |p| (p.0.len() as u64, p.1.len() as u64));
            if places != cell.placements {
                return Err(format!(
                    "light index: cell {} ({}) yielded {} placements from its segments, the inventory counts {}",
                    ci, cell.name, places, cell.placements
                ));
            }
            if texts && recs != cell.texts {
                return Err(format!(
                    "light index: cell {} ({}) yielded {} texts from its segments, the inventory counts {}",
                    ci, cell.name, recs, cell.texts
                ));
            }
        }
        Ok((pieces_of, tasks.len()))
    }

    /// Every cell's placements (`placements_of` over all cells; a cell
    /// without any gets an empty list): the occupancy pass's instances
    /// (`floe-index light --occupancy`), read from the placement-flagged
    /// segments alone - not the whole file.
    pub fn all_placements(&self, jobs: usize) -> Result<Vec<Vec<PlaceRec>>, String> {
        let n = self.inv.cells.len();
        let cells: Vec<u32> = (0..n as u32).collect();
        let (mut by_cell, _) = self.placements_of(&cells, jobs)?;
        Ok((0..n as u32).map(|ci| by_cell.remove(&ci).unwrap_or_default()).collect())
    }

    /// Whether the index holds cell `ci`'s placements and texts (always for
    /// a cell without any).
    pub fn is_filled(&self, ci: u32) -> bool {
        let f = self.fills.lock().unwrap_or_else(|p| p.into_inner());
        (ci as usize) < self.inv.cells.len()
            && (!needs_fill(&self.inv.cells[ci as usize]) || f.cells[ci as usize].is_some())
    }

    /// a cell's placements as the full build's build_cell_plan encodes them:
    /// the items' boxes (the child's recursive box under the placement,
    /// widened by the repetition), the v7 size annotations, the instance BVH
    /// (floe_ovm::split_bvh) and the records in leaf order; its texts kept
    /// as they are (the image encodes them, `build_cell_texts`)
    fn encode_places(&self, places: &[PlaceRec], texts: Vec<TextRec>) -> CellFill {
        let sat32 = |v: i64| u32::try_from(v.max(0)).unwrap_or(u32::MAX);
        let rbb = |ci: usize| -> BBox {
            match self.inv.rbb.get(ci).copied().flatten() {
                Some(b) => bbox_of(&b),
                None => BBox::EMPTY,
            }
        };
        let mut items: Vec<(BBox, u32, u32, usize)> = places
            .iter()
            .enumerate()
            .map(|(pi, pl)| {
                let cb = rbb(pl.cell);
                let (cw, ch) = if cb.is_empty() {
                    (0, 0)
                } else {
                    ((cb.x1 - cb.x0).max(0), (cb.y1 - cb.y0).max(0))
                };
                let (md, mn) = (sat32(cw.max(ch)), sat32(cw.min(ch)));
                let bb = if cb.is_empty() {
                    BBox {
                        x0: pl.x,
                        y0: pl.y,
                        x1: pl.x,
                        y1: pl.y,
                    }
                } else {
                    let xf = Xf::place(pl.x, pl.y, pl.rot, pl.flip);
                    let a = xf.apply(cb.x0, cb.y0);
                    let c = xf.apply(cb.x1, cb.y1);
                    let (ex, ey) = rep_extent(&pl.rep);
                    BBox {
                        x0: a.0.min(c.0) + ex.0.min(0),
                        y0: a.1.min(c.1) + ey.0.min(0),
                        x1: a.0.max(c.0) + ex.1.max(0),
                        y1: a.1.max(c.1) + ey.1.max(0),
                    }
                };
                (bb, md, mn, pi)
            })
            .collect();
        let mut sink = CellSink::default();
        if !items.is_empty() {
            let mut nodes: Vec<(BBox, u32, u16, bool, u32, u32)> =
                vec![(BBox::EMPTY, 0, 0, false, 0, 0)];
            split_bvh(&mut nodes, &mut items, 0, 0);
            for &(.., pi) in &items {
                let pl = &places[pi];
                match &pl.rep {
                    Rep::Pts(p) => {
                        sink.place_pts(pl.cell as u32, pl.x, pl.y, pl.rot, pl.flip, &prepare_pts(p));
                    }
                    rep => {
                        sink.place(pl.cell as u32, pl.x, pl.y, pl.rot, pl.flip, rep);
                    }
                }
            }
            for &(bb, first, count, leaf, md, mn) in &nodes {
                sink.bvh_node(&bb, first, count, leaf, md, mn);
            }
        }
        // (a text's share of the budget: its record, its string, its points)
        let text_bytes: u64 = texts
            .iter()
            .map(|t| {
                80 + t.s.len() as u64
                    + match &t.rep {
                        Rep::Pts(p) => 16 * p.len() as u64,
                        _ => 0,
                    }
            })
            .sum();
        let bytes = (sink.places.len() + sink.pts_pool.len() + sink.bvh.len()) as u64 + text_bytes;
        CellFill {
            places: sink.places,
            n_places: sink.n_places,
            pool: sink.pts_pool,
            bvh: sink.bvh,
            n_bvh: sink.n_bvh,
            texts,
            bytes,
            serial: 0,
        }
    }

    // --------------------------------------------------- the image

    /// The index image with every filled cell's placements and texts, as a
    /// validated `Ovm` marked light (floe_ovm LightMarks: the cells with
    /// placements or texts not filled are the unfilled ones), and the
    /// image's design.ovt bytes (the filled cells' strings and text point
    /// lists).
    pub fn ovm(&self) -> Result<(Ovm, Vec<u8>), String> {
        let image = self.image()?;
        let mut ovm = Ovm::from_bytes_shallow(image.bytes)?;
        ovm.set_light_marks(&image.unfilled_places, &image.unfilled_texts);
        Ok((ovm, image.ovt))
    }

    /// the image's bytes, its design.ovt bytes and, per cell, whether its
    /// placements / its texts are missing
    pub fn image(&self) -> Result<LightImage, String> {
        let inv = &self.inv;
        let n = inv.cells.len();
        let nl = inv.layer_order.len();
        let mut b = Builder::new(inv.unit(), self.identity.src_size, self.identity.src_mtime, nl);
        b.top = inv.top as u32;
        let f = self.fills.lock().unwrap_or_else(|p| p.into_inner());
        let mut unfilled_places = vec![false; n];
        let mut unfilled_texts = vec![false; n];
        let mut ovt: Vec<u8> = Vec::new();
        for ci in 0..n {
            let st = &self.cell_static[ci];
            let mut sink = CellSink {
                pbvh: st.pbvh.clone(),
                n_pbvh: st.n_pbvh,
                pranges: st.pranges.clone(),
                n_pranges: st.n_pranges,
                ..CellSink::default()
            };
            let mut texts: &[TextRec] = &[];
            match &f.cells[ci] {
                Some(c) => {
                    sink.places.clone_from(&c.places);
                    sink.n_places = c.n_places;
                    sink.pts_pool.clone_from(&c.pool);
                    sink.bvh.clone_from(&c.bvh);
                    sink.n_bvh = c.n_bvh;
                    texts = &c.texts;
                }
                None => {
                    unfilled_places[ci] = inv.cells[ci].placements > 0;
                    unfilled_texts[ci] = inv.cells[ci].texts > 0;
                }
            }
            let (page_base, page_count) = self.cell_pages[ci];
            let bases = b.append_cell_sink(&sink, page_base as u64);
            let (trange_start, trange_count) =
                build_cell_texts(&mut b, &mut ovt, ci as u32, texts, &self.layer_index)?;
            for p in &self.pages[page_base as usize..(page_base + page_count) as usize] {
                let s = &self.seg.segs[p.seg as usize];
                b.page(
                    ci as u32,
                    p.layer_idx,
                    p.seq,
                    &p.bbox,
                    s.offset,
                    p.csize as u64,
                    p.usize_ as u64,
                    p.records as u64,
                    p.members,
                    p.max_w,
                    p.max_h,
                    LOD_EXACT,
                    LOD_PAGE_NONE,
                );
                b.page_max_min(p.max_min);
                b.page_codec(CODEC_SEGMENT);
            }
            let cell = &inv.cells[ci];
            let mask_d = b.bitset(&cell.dmask);
            let mask_r = b.bitset(&inv.rmask[ci]);
            let mask_t = b.bitset(&inv.rtmask[ci]);
            let rbbox = inv.rbb[ci].map(|r| bbox_of(&r)).unwrap_or(BBox::EMPTY);
            let (bvh_start, bvh_count) = if sink.n_bvh == 0 {
                (0, 0)
            } else {
                (bases.bvh_start, sink.n_bvh)
            };
            b.cell(
                &cell.name,
                inv.height[ci],
                inv.rank[ci],
                &bbox_of(&cell.dbox),
                &rbbox,
                floe_ovm::narrow_u32(bases.place_start, "place count"),
                floe_ovm::narrow_u32(sink.n_places, "cell place count"),
                page_base,
                page_count,
                bvh_start,
                bvh_count,
                bases.prange_start,
                sink.n_pranges,
                mask_d,
                mask_r,
                inv.rmembers[ci],
                trange_start,
                trange_count,
                mask_t,
            );
        }
        drop(f);
        for (i, &(l, d)) in inv.layer_order.iter().enumerate() {
            let nm = inv
                .layer_names
                .get(&(l, d))
                .cloned()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("{}/{}", l, d));
            b.layer(l, d, &nm, inv.lrecs[i], inv.lmems[i]);
        }
        let jobs = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
        b.annotate_bvh_masks(jobs);
        // ovp_len = the source's size: every page's span lies inside it;
        // ovt_len = the bytes made here
        let bytes = b.finish(self.identity.src_size, ovt.len() as u64);
        Ok(LightImage {
            bytes,
            ovt,
            unfilled_places,
            unfilled_texts,
        })
    }

    // ------------------------------------------------- the summary

    /// The hierarchy summary (hiersum, design.ovh's bytes) from the
    /// inventory's edges: per parent its distinct children with their
    /// placed members and the union of the child's recursive box under
    /// the parent's placements of it - what `hiersum::build` reads off a
    /// full index's placement records.
    pub fn summary(&self) -> (Vec<u8>, hiersum::BuildStats) {
        let inv = &self.inv;
        let n = inv.cells.len();
        let mut kids_of: Vec<Vec<(u32, u64, BBox)>> = Vec::with_capacity(n);
        let mut n_places = 0u64;
        for c in &inv.cells {
            n_places = n_places.saturating_add(c.placements);
            let mut kids: Vec<(u32, u64, BBox)> = c
                .edges
                .iter()
                .map(|e| {
                    let mut ext = EMPTY_BOX;
                    if let Some(cb) = inv.rbb.get(e.child).copied().flatten() {
                        for (o, ob) in e.obox.iter().enumerate() {
                            if box_is_empty(ob) {
                                continue;
                            }
                            let r = orient_box((o & 3) as u8, o & 4 != 0, cb);
                            let placed = (r.0 + ob.0, r.1 + ob.1, r.2 + ob.2, r.3 + ob.3);
                            floe_oasis::geom::box_grow(&mut ext, &placed);
                        }
                    }
                    (e.child as u32, e.members, bbox_of(&ext))
                })
                .collect();
            kids.sort_unstable_by_key(|k| k.0);
            kids_of.push(kids);
        }
        hiersum::build_from_edges(
            n as u32,
            n_places,
            n_places,
            self.identity.src_size,
            self.identity.src_mtime,
            inv.top as u32,
            &inv.rank,
            &kids_of,
        )
    }

    /// The deepest placement level holding own shapes of each layer (the
    /// full index's render-core layer_max_depths), from the inventory's
    /// children lists: the longest top-to-cell path of every cell, then per
    /// layer the maximum over the cells whose own mask holds it.
    pub fn layer_max_depths(&self) -> Vec<u32> {
        let inv = &self.inv;
        let n = inv.cells.len();
        let nl = inv.layer_order.len();
        if n == 0 || inv.top >= n {
            return vec![0; nl];
        }
        let children = inv.children();
        let mut state = vec![0u8; n];
        let mut order: Vec<usize> = Vec::new();
        let mut stack: Vec<(usize, usize)> = vec![(inv.top, 0)];
        state[inv.top] = 1;
        while let Some(frame) = stack.last_mut() {
            let (ci, k) = *frame;
            if k < children[ci].len() {
                frame.1 += 1;
                let child = children[ci][k];
                if child < n && state[child] == 0 {
                    state[child] = 1;
                    stack.push((child, 0));
                }
            } else {
                stack.pop();
                state[ci] = 2;
                order.push(ci);
            }
        }
        let mut longest = vec![u32::MAX; n];
        longest[inv.top] = 0;
        for &ci in order.iter().rev() {
            let d = longest[ci];
            if d == u32::MAX {
                continue;
            }
            for &k in &children[ci] {
                if k < n && (longest[k] == u32::MAX || longest[k] < d + 1) {
                    longest[k] = d + 1;
                }
            }
        }
        let mut out = vec![0u32; nl];
        for ci in 0..n {
            let d = longest[ci];
            if d == u32::MAX {
                continue;
            }
            let bits = &inv.cells[ci].dmask;
            for (li, slot) in out.iter_mut().enumerate() {
                let byte = bits.get(li / 8).copied().unwrap_or(0);
                if (byte >> (li % 8)) & 1 == 1 && *slot < d {
                    *slot = d;
                }
            }
        }
        out
    }
}

/// What `write_files` wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// One assembled image of a light index (`LightIndex::image`).
pub struct LightImage {
    /// the design.ovm image
    pub bytes: Vec<u8>,
    /// the image's design.ovt bytes: the filled cells' text strings and
    /// the point lists of their Pts repetitions
    pub ovt: Vec<u8>,
    /// per cell: the inventory counts placements the image lacks
    pub unfilled_places: Vec<bool>,
    /// per cell: the inventory counts texts the image lacks
    pub unfilled_texts: Vec<bool>,
}

/// A filled cell's texts into the image as the full build's
/// build_cell_texts (cli vfs.rs) writes them: per layer a run in Morton
/// order over the texts' boxes (source order the tie-break) with a text BVH
/// past TBVH_LEAF (floe_ovm::split_tbvh, the storage order its leaves'),
/// the strings and the Morton-ordered point lists of Pts repetitions
/// appended to `ovt` in source order, a text's box its anchor grown by the
/// repetition's extent. Returns the cell's (trange_start, trange_count); a
/// cell without texts gets (the next run index, 0), as the full build
/// writes it.
fn build_cell_texts(
    b: &mut Builder,
    ovt: &mut Vec<u8>,
    ci: u32,
    texts: &[TextRec],
    lidx: &HashMap<(u32, u32), u32>,
) -> Result<(u32, u32), String> {
    let tr_start = b.n_tranges();
    if texts.is_empty() {
        return Ok((tr_start, 0));
    }
    let narrow = |v: u64, what: &str| -> Result<u32, String> {
        u32::try_from(v).map_err(|_| format!("light index: {} {} is more than the index can hold", what, v))
    };
    let add = |a: i64, b: i64, what: &str| -> Result<i64, String> {
        a.checked_add(b)
            .ok_or_else(|| format!("light index: {} overflows", what))
    };
    let mut groups: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (ti, t) in texts.iter().enumerate() {
        let li = *lidx.get(&(t.layer, t.dt)).ok_or_else(|| {
            format!(
                "light index: text layer {}/{} is not in the inventory's layer table",
                t.layer, t.dt
            )
        })?;
        groups.entry(li).or_default().push(narrow(ti as u64, "cell text count")?);
    }
    struct TRec {
        bbox: BBox,
        rep: u32,
        soff: u64,
        slen: u32,
        seq: u32,
        x: i64,
        y: i64,
    }
    for (li, idxs) in groups {
        // strings and reps in SOURCE order (the ovt byte layout), the run
        // records ordered spatially
        let mut recs: Vec<TRec> = Vec::with_capacity(idxs.len());
        for &ti in &idxs {
            let t = &texts[ti as usize];
            let soff = ovt.len() as u64;
            ovt.extend_from_slice(t.s.as_bytes());
            let slen = narrow(t.s.len() as u64, "text string length")?;
            let rep = match &t.rep {
                Rep::One => TREP_NONE,
                Rep::Grid { na, nb, va, vb } => {
                    b.trep_grid(narrow(*na, "text rep na")?, narrow(*nb, "text rep nb")?, *va, *vb)
                }
                Rep::Pts(p) => {
                    let prep = prepare_pts(p);
                    let pts_off = ovt.len() as u64;
                    for &(px, py) in &prep.pts {
                        ovt.extend_from_slice(&px.to_le_bytes());
                        ovt.extend_from_slice(&py.to_le_bytes());
                    }
                    let chunk_lo = b.n_tchunks();
                    for c in &prep.chunks {
                        b.tchunk(c);
                    }
                    b.trep_pts(
                        narrow(prep.pts.len() as u64, "text pts count")?,
                        pts_off,
                        chunk_lo,
                        narrow(prep.chunks.len() as u64, "text pts chunks")?,
                    )
                }
            };
            let (ex, ey) = rep_extent(&t.rep);
            recs.push(TRec {
                bbox: BBox {
                    x0: add(t.x, ex.0.min(0), "text bbox x0")?,
                    y0: add(t.y, ey.0.min(0), "text bbox y0")?,
                    x1: add(t.x, ex.1.max(0), "text bbox x1")?,
                    y1: add(t.y, ey.1.max(0), "text bbox y1")?,
                },
                rep,
                soff,
                slen,
                seq: ti,
                x: t.x,
                y: t.y,
            });
        }
        // Morton pre-sort over bbox centers, source_seq tie-break
        let center = |r: &TRec| {
            (
                ((r.bbox.x0 as i128 + r.bbox.x1 as i128) / 2) as i64,
                ((r.bbox.y0 as i128 + r.bbox.y1 as i128) / 2) as i64,
            )
        };
        let (mut minx, mut miny) = (i64::MAX, i64::MAX);
        for r in &recs {
            let (cx, cy) = center(r);
            minx = minx.min(cx);
            miny = miny.min(cy);
        }
        let mut order: Vec<u32> = (0..recs.len() as u32).collect();
        order.sort_by_key(|&i| {
            let (cx, cy) = center(&recs[i as usize]);
            (morton_key(cx, cy, minx, miny), recs[i as usize].seq)
        });
        let text_lo = b.n_texts();
        let count = narrow(recs.len() as u64, "trange text count")?;
        let root = if recs.len() <= TBVH_LEAF {
            for &i in &order {
                let r = &recs[i as usize];
                b.text(ci, li, r.x, r.y, r.soff, r.slen, r.rep, &r.bbox, r.seq);
            }
            TBVH_NONE
        } else {
            // the BVH partition decides the storage order (leaves are
            // contiguous item ranges); the texts go out in that order
            let mut items: Vec<(BBox, u32)> = order.iter().map(|&i| (recs[i as usize].bbox, i)).collect();
            let mut nodes: Vec<(BBox, u32, u16, bool)> = vec![(BBox::EMPTY, 0, 0, false)];
            split_tbvh(&mut nodes, &mut items, 0, 0);
            for &(_, i) in &items {
                let r = &recs[i as usize];
                b.text(ci, li, r.x, r.y, r.soff, r.slen, r.rep, &r.bbox, r.seq);
            }
            let base = b.n_tbvh();
            for &(bbb, first, cnt, leaf) in &nodes {
                let f = if leaf {
                    narrow(text_lo as u64 + first as u64, "text index")?
                } else {
                    narrow(base as u64 + first as u64, "tbvh index")?
                };
                b.tbvh_node(&bbb, f, cnt, leaf);
            }
            base
        };
        b.trange(li, text_lo, count, root);
    }
    Ok((tr_start, b.n_tranges() - tr_start))
}

pub struct Written {
    pub cells: usize,
    pub layers: usize,
    pub records: u64,
    pub segments: usize,
    pub cblocks: usize,
    pub layer_rows: usize,
    pub inv_bytes: usize,
    pub seg_bytes: usize,
    /// segments holding a CELL record inside a CBLOCK (the renderer refuses
    /// the light cache then)
    pub cells_inside: usize,
}

/// The light cache's index files for the source bytes `data` (identity
/// `id`) into `dir`: one inventory pass with the segment directory
/// (floe_oasis::inventory_segments) writes design.inv and design.seg,
/// paired, each by tmp + rename. `floe-index light` adds meta.json.
pub fn write_files(
    data: &[u8],
    id: Identity,
    dir: &str,
    jobs: usize,
    span_bytes: u64,
) -> Result<Written, String> {
    let (inv, col) = floe_oasis::inventory_segments(data, jobs, span_bytes).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {}", dir, e))?;
    let publish = |name: &str, bytes: &[u8]| -> Result<(), String> {
        let path = Path::new(dir).join(name);
        let tmp = Path::new(dir).join(format!("{}.tmp", name));
        std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {}", tmp.display(), e))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {}", path.display(), e))
    };
    let inv_bytes = invfile::encode(&inv, id);
    publish(invfile::FILE_NAME, &inv_bytes)?;
    let mut f = SegFile::from_segments(&col.segs, &inv, span_bytes).map_err(|e| e.to_string())?;
    f.inv_crc = segfile::inv_crc_of(&inv_bytes).unwrap_or(0);
    let seg_bytes = segfile::encode(&f, id);
    publish(segfile::FILE_NAME, &seg_bytes)?;
    Ok(Written {
        cells: inv.cells.len(),
        layers: inv.layer_order.len(),
        records: inv.scan.records,
        segments: f.segs.len(),
        cblocks: f.segs.iter().filter(|s| s.is_cblock()).count(),
        layer_rows: f.layers.len(),
        inv_bytes: inv_bytes.len(),
        seg_bytes: seg_bytes.len(),
        cells_inside: col.segs.iter().filter(|s| s.cells_inside > 0).count(),
    })
}

/// a segment's bytes apportioned to one layer's row by its records: the
/// page's `csize` (file bytes) and `usize_` (record bytes) - a one-layer
/// segment's whole, a many-layer segment's share, so the planner's budget
/// fit and the page cache's charge see about the bytes the layer costs
fn apportion(s: &SegRow, row_records: u64) -> Result<(u32, u32), String> {
    let share = |total: u64| -> u64 {
        let shapes = s.shapes.max(1) as u64;
        if row_records >= shapes {
            total
        } else {
            ((total as u128 * row_records as u128).div_ceil(shapes as u128) as u64).clamp(1, total.max(1))
        }
    };
    let narrow = |v: u64, what: &str| {
        u32::try_from(v).map_err(|_| {
            format!(
                "light index: segment at {} has {} {} bytes, more than a page can hold",
                s.offset, v, what
            )
        })
    };
    let inflated = if s.inflated == 0 { s.bytes } else { s.inflated };
    Ok((narrow(share(s.bytes), "file")?, narrow(share(inflated), "record")?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hiersum;
    use floe_oasis::doc::{parse_doc, PathRec, PolyRec, RectRec};
    use floe_oasis::inventory::fixtures::{adversarial, head_cblock_cell, labelled_tree};
    use floe_oasis::segments::DEFAULT_SPAN_BYTES;
    use floe_oasis::write::{write_tree, WCell};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    /// a fresh directory under the temp dir
    fn scratch(tag: &str) -> std::path::PathBuf {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("floe-light-{}-{}-{}", std::process::id(), n, tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// `data` as `dir/src.oas` and its light files in `dir/.src.oas.ice`
    /// (the sibling rule names the source): (source path, light dir)
    fn light_of(tag: &str, data: &[u8]) -> (String, String) {
        let d = scratch(tag);
        let src = d.join("src.oas");
        std::fs::write(&src, data).unwrap();
        let src = src.to_string_lossy().into_owned();
        let id = file_identity(&src).unwrap();
        let dir = d.join(".src.oas.ice").to_string_lossy().into_owned();
        write_files(data, id, &dir, 1, DEFAULT_SPAN_BYTES).unwrap();
        (src, dir)
    }

    fn rect(layer: u32, x: i64, y: i64, w: i64, h: i64, rep: Rep) -> RectRec {
        RectRec { layer, dt: 0, x, y, w, h, rep }
    }

    /// a three-level tree written by the crate's writer: LEAF (a rect on
    /// 1/0 and a path on 3/0), MID (a polygon on 2/0, LEAF once and as a
    /// 2x3 grid), TOP (a rect on 1/0, MID at the origin and turned at
    /// (500, 100), LEAF flipped as a point list)
    fn tree() -> Vec<u8> {
        let leaf_rects = [rect(1, 0, 0, 10, 4, Rep::One)];
        let leaf_paths = [PathRec {
            layer: 3,
            dt: 0,
            pts: vec![(0, 2), (10, 2)],
            hw: 1,
            es: 0,
            ee: 0,
            rep: Rep::One,
        }];
        let mid_polys = [PolyRec {
            layer: 2,
            dt: 0,
            pts: vec![(0, 0), (100, 0), (100, 100), (0, 100)],
            rep: Rep::One,
        }];
        let top_rects = [rect(1, 0, 0, 1000, 1000, Rep::One)];
        let grid = Rep::Grid {
            na: 2,
            nb: 3,
            va: (30, 0),
            vb: (0, 10),
        };
        let pts = Rep::Pts(vec![(0, 0), (50, 0), (0, 50)].into());
        let one = Rep::One;
        let cells = [
            WCell {
                name: "LEAF".into(),
                rects: &leaf_rects,
                polys: &[],
                paths: &leaf_paths,
                texts: &[],
                places: vec![],
            },
            WCell {
                name: "MID".into(),
                rects: &[],
                polys: &mid_polys,
                paths: &[],
                texts: &[],
                places: vec![("LEAF", 5, 5, 0, false, &one), ("LEAF", 20, 50, 0, false, &grid)],
            },
            WCell {
                name: "TOP".into(),
                rects: &top_rects,
                polys: &[],
                paths: &[],
                texts: &[],
                places: vec![
                    ("MID", 0, 0, 0, false, &one),
                    ("MID", 500, 100, 1, false, &one),
                    ("LEAF", 800, 800, 0, true, &pts),
                ],
            },
        ];
        write_tree(&cells, 1000.0).unwrap()
    }

    fn streams() -> Vec<(&'static str, Vec<u8>)> {
        vec![
            ("tree", tree()),
            ("adv", adversarial(false, false)),
            ("adv tables first", adversarial(true, false)),
            ("adv cblock", adversarial(false, true)),
            ("adv tables first cblock", adversarial(true, true)),
        ]
    }

    /// own members per (cell name, layer) of a Doc
    fn members_of(doc: &floe_oasis::doc::Doc) -> std::collections::BTreeMap<(String, (u32, u32)), u64> {
        let mut m = std::collections::BTreeMap::new();
        for c in &doc.cells {
            for r in &c.rects {
                *m.entry((c.name.clone(), (r.layer, r.dt))).or_insert(0) += r.rep.members();
            }
            for p in &c.polys {
                *m.entry((c.name.clone(), (p.layer, p.dt))).or_insert(0) += p.rep.members();
            }
            for p in &c.paths {
                *m.entry((c.name.clone(), (p.layer, p.dt))).or_insert(0) += p.rep.members();
            }
        }
        m
    }

    #[test]
    fn a_light_index_opens_fills_and_its_summary_is_the_full_ones() {
        for (name, data) in streams() {
            let (_src, dir) = light_of("open", &data);
            let li = LightIndex::open(&dir).unwrap_or_else(|e| panic!("{}: {}", name, e));
            let n = li.inv.cells.len() as u32;
            let placed: Vec<u32> = (0..n).filter(|&ci| li.inv.cells[ci as usize].placements > 0).collect();
            // the cells a fill is for: with placements, or with texts (L6)
            let needed: Vec<u32> = (0..n).filter(|&ci| needs_fill(&li.inv.cells[ci as usize])).collect();
            // the first image: every cell with placements or texts unfilled,
            // the planner's touch recorded for the placements, nothing on a
            // cell without any
            let (ovm, ovt) = li.ovm().unwrap_or_else(|e| panic!("{}: image: {}", name, e));
            assert!(ovm.is_light() && ovt.is_empty());
            assert_eq!(ovm.n_cells, n, "{}", name);
            assert_eq!(ovm.n_places, 0, "{}", name);
            assert_eq!(ovm.n_pages, li.n_pages(), "{}", name);
            for ci in 0..n {
                assert_eq!(ovm.light_unfilled(ci), needed.contains(&ci), "{}: cell {}", name, ci);
                assert!(!li.is_filled(ci) || !needed.contains(&ci), "{}: cell {}", name, ci);
                let _ = ovm.cell_places(ci);
            }
            assert_eq!(ovm.light_touched(), placed, "{}: the accessor marks the unfilled cells", name);
            // the page rows: codec, cell, layer, bytes within the source
            for pi in 0..ovm.n_pages {
                let p = ovm.page(pi);
                let lp = li.page(pi).unwrap();
                assert_eq!(p.codec, CODEC_SEGMENT, "{}", name);
                assert_eq!(p.layer_idx, lp.layer_idx, "{}", name);
                assert_eq!(p.cell, li.segment(lp.seg).cell, "{}", name);
                assert!(p.file_off + p.csize as u64 <= data.len() as u64, "{}", name);
                assert!(p.records >= 1 && p.usize_ >= 1, "{}", name);
            }
            // the prefill set from the top: everything placed below it within
            // the caps, the top alone without
            let top = li.inv.top as u32;
            let mut set = li.fill_set_within(&[top], 1 << 20, 1 << 30);
            set.sort_unstable();
            let mut want: Vec<u32> = needed.clone();
            if !want.contains(&top) {
                want.push(top);
                want.sort_unstable();
            }
            assert_eq!(set, want, "{}: prefill set", name);
            assert_eq!(li.fill_set_within(&[top], 0, 0), vec![top], "{}", name);
            // fill everything: the image's placements are the source's, no
            // mark is left, and the summary made from the image's placements
            // (what a full index gets) is the one made from the inventory
            let report = li.fill(&(0..n).collect::<Vec<_>>()).unwrap_or_else(|e| panic!("{}: fill: {}", name, e));
            assert_eq!(report.cells, needed.len(), "{}", name);
            assert_eq!(
                report.placements,
                li.inv.cells.iter().map(|c| c.placements).sum::<u64>(),
                "{}",
                name
            );
            assert_eq!(li.fill(&[top]).unwrap(), FillReport::default(), "{}: a second fill is nothing", name);
            let (ovm, _) = li.ovm().unwrap();
            assert_eq!(ovm.n_places, report.placements, "{}", name);
            for ci in 0..n {
                let _ = ovm.cell_places(ci);
                let _ = ovm.cell_bvh(ci);
                assert!(li.is_filled(ci), "{}: {}", name, ci);
            }
            assert!(ovm.light_touched().is_empty(), "{}", name);
            let (from_image, st) = hiersum::build(&ovm);
            let (from_edges, st2) = li.summary();
            assert_eq!(from_image, from_edges, "{}: summary bytes", name);
            assert_eq!(st, st2, "{}: summary stats", name);
            let s = hiersum::HierSummary::from_bytes(from_edges).unwrap();
            s.validate_against(&ovm).unwrap();
            // the deep validation of the assembled image passes too
            floe_ovm::Ovm::from_bytes(li.image().unwrap().bytes).unwrap_or_else(|e| panic!("{}: deep: {}", name, e));
            // the layer depths: the top's own layers at 0
            let depths = li.layer_max_depths();
            assert_eq!(depths.len(), li.inv.layer_order.len(), "{}", name);
        }
    }

    #[test]
    fn every_page_decodes_to_its_layers_shapes_of_its_cell() {
        for (name, data) in streams() {
            let (_src, dir) = light_of("pages", &data);
            let li = LightIndex::open(&dir).unwrap();
            let doc = parse_doc(&data).unwrap();
            let want = members_of(&doc);
            let mut got: std::collections::BTreeMap<(String, (u32, u32)), u64> = std::collections::BTreeMap::new();
            for pi in 0..li.n_pages() {
                let lp = li.page(pi).unwrap();
                let seg = li.segment(lp.seg);
                let cell = li.segment_cell(lp.seg).unwrap_or_else(|e| panic!("{}: {}", name, e));
                let key = li.layer_key(lp.layer_idx);
                let (mut records, mut members) = (0u32, 0u64);
                for r in &cell.rects {
                    if (r.layer, r.dt) == key {
                        records += 1;
                        members += r.rep.members();
                    }
                }
                for p in &cell.polys {
                    if (p.layer, p.dt) == key {
                        records += 1;
                        members += p.rep.members();
                    }
                }
                for p in &cell.paths {
                    if (p.layer, p.dt) == key {
                        records += 1;
                        members += p.rep.members();
                    }
                }
                assert_eq!(records, lp.records, "{}: page {} records", name, pi);
                assert_eq!(members, lp.members, "{}: page {} members", name, pi);
                let cname = li.inv.cells[seg.cell as usize].name.clone();
                *got.entry((cname, key)).or_insert(0) += members;
            }
            assert_eq!(got, want, "{}: own members per cell and layer", name);
        }
    }

    #[test]
    fn texts_are_filled_with_the_placements_and_planned_as_labels() {
        let data = labelled_tree();
        let (src, dir) = light_of("texts", &data);
        let li = LightIndex::open(&dir).unwrap();
        let doc = parse_doc(&data).unwrap();
        let n = li.inv.cells.len() as u32;
        let name = |ci: u32| li.inv.cells[ci as usize].name.as_str();
        let txt = (0..n).find(|&ci| name(ci) == "TXT").unwrap();
        let leaf = (0..n).find(|&ci| name(ci) == "LEAF").unwrap();
        assert_eq!(li.inv.cells[txt as usize].placements, 0);
        assert_eq!(li.inv.cells[txt as usize].texts, 1);
        // the first image: no texts; TXT (texts alone) and LEAF (a text, no
        // placements) are unfilled for the label walk's touch alone - a
        // placement reader passing them asks for nothing
        let (ovm, ovt) = li.ovm().unwrap();
        assert!(ovt.is_empty() && ovm.ovt_len == 0);
        assert!(ovm.light_unfilled(txt) && ovm.light_unfilled(leaf));
        assert!(!li.is_filled(txt));
        let _ = ovm.cell_places(txt);
        let _ = ovm.cell_bvh(leaf);
        assert!(ovm.light_touched().is_empty(), "a placement reader does not ask for a cell of texts alone");
        let _ = ovm.cell_tranges(txt);
        assert_eq!(ovm.light_touched(), vec![txt]);
        assert_eq!(ovm.cell_tranges(txt), (ovm.cell_tranges(txt).0, 0));
        // the prefill set from the top takes the cells of texts alone too
        let top = li.inv.top as u32;
        let mut set = li.fill_set_within(&[top], 1 << 20, 1 << 30);
        set.sort_unstable();
        assert_eq!(set, (0..n).collect::<Vec<_>>());
        // fill everything: per cell and layer the image's texts are the
        // Doc's - anchors, strings and repetition members
        let report = li.fill(&(0..n).collect::<Vec<_>>()).unwrap();
        assert_eq!(report.cells, n as usize, "every cell has a placement or a text");
        assert_eq!(report.texts, doc.cells.iter().map(|c| c.texts.len() as u64).sum::<u64>());
        assert_eq!(report.texts, 5);
        let (ovm, ovt) = li.ovm().unwrap();
        assert!(ovm.light_touched().is_empty());
        assert_eq!(ovm.ovt_len, ovt.len() as u64);
        for ci in 0..n {
            assert!(li.is_filled(ci));
            let cell = &doc.cells[ci as usize];
            assert_eq!(cell.name, name(ci));
            let mut want: Vec<((u32, u32), i64, i64, String, u64)> = cell
                .texts
                .iter()
                .map(|t| ((t.layer, t.dt), t.x, t.y, t.s.clone(), t.rep.members()))
                .collect();
            want.sort();
            let (ts, tc) = ovm.cell_tranges(ci);
            let mut got = Vec::new();
            for tri in ts..ts + tc {
                let tr = ovm.trange(tri);
                for ti in tr.text_lo..tr.text_lo + tr.text_count {
                    let t = ovm.text(ti);
                    assert_eq!(t.cell, ci);
                    let s = std::str::from_utf8(&ovt[t.string_off as usize..(t.string_off + t.string_len as u64) as usize])
                        .unwrap()
                        .to_string();
                    let members = if t.rep_idx == TREP_NONE {
                        1
                    } else {
                        match ovm.trep(t.rep_idx) {
                            floe_ovm::TrepV::Grid { na, nb, .. } => na as u64 * nb as u64,
                            floe_ovm::TrepV::Pts { count, .. } => count as u64,
                        }
                    };
                    got.push((li.layer_key(tr.layer_idx), t.x, t.y, s, members));
                }
            }
            got.sort();
            assert_eq!(got, want, "cell {}", name(ci));
        }
        // the deep validation reads the text sections and the pts pools
        // against ovt_len
        floe_ovm::Ovm::from_bytes(li.image().unwrap().bytes).unwrap();
        // the raw label walk over the filled image: every placed text
        // member of the flattened tree
        let vfs = crate::Vfs::light(ovm, src.clone(), ovt);
        let req = crate::ViewReq {
            view: BBox {
                x0: -10_000,
                y0: -10_000,
                x1: 10_000,
                y1: 10_000,
            },
            cut_dbu: 0,
            vis: vec![0xff; vfs.ovm.bs_width],
            depth: u32::MAX,
            px_per_dbu: 0.0,
            sub_cut_wash: false,
            page_reps: false,
            decode_budget: 0,
            page_hairline: false,
            page_skip: Vec::new(),
            prune_skipped: false,
            sub_cut_box: false,
            shape_cut: false,
            shape_cut_max: false,
            frames: true,
            root: None,
            page_wash: true,
            lod_swap: true,
        };
        let opts = crate::text::LabelOpts {
            raw: true,
            ..Default::default()
        };
        let plan = crate::text::plan_labels(&vfs.ovm, vfs.ovt(), &req, &opts).unwrap();
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for r in plan.rows.iter().filter(|r| !r.block) {
            *counts.entry(r.s.clone()).or_insert(0) += 1;
        }
        let want: BTreeMap<String, usize> = [("L", 17), ("M", 8), ("N", 6), ("T", 1), ("X", 1)]
            .into_iter()
            .map(|(s, k)| (s.to_string(), k))
            .collect();
        assert_eq!(counts, want);
        assert!(plan.rows.iter().any(|r| r.s == "X" && (r.x, r.y) == (907, 907)), "{:?}", plan.rows);
        assert!(plan.rows.iter().any(|r| r.s == "T" && (r.x, r.y) == (500, 990)));
        // the fills do not change when asked again, and the placements are
        // the inventory's
        assert_eq!(li.fill(&[top, txt]).unwrap(), FillReport::default());
        assert_eq!(report.placements, li.inv.cells.iter().map(|c| c.placements).sum::<u64>());
    }

    #[test]
    fn what_the_light_index_refuses() {
        // a cell defined inside a CBLOCK: the directory cannot tell its
        // records from the host's
        let (_src, dir) = light_of("head", &head_cblock_cell());
        let e = LightIndex::open(&dir).unwrap_err();
        assert!(e.contains("inside CBLOCKs"), "{}", e);
        // the source changed since the files were made
        let (src, dir) = light_of("changed", &tree());
        let mut data = tree();
        data.push(0);
        std::fs::write(&src, &data).unwrap();
        let e = LightIndex::open(&dir).unwrap_err();
        assert!(e.contains("is not the source"), "{}", e);
        // a directory from another source's inventory
        let (_a, dir_a) = light_of("pair-a", &tree());
        let (_b, dir_b) = light_of("pair-b", &adversarial(false, false));
        std::fs::copy(
            Path::new(&dir_b).join(segfile::FILE_NAME),
            Path::new(&dir_a).join(segfile::FILE_NAME),
        )
        .unwrap();
        let e = LightIndex::open(&dir_a).unwrap_err();
        assert!(e.contains("another source") || e.contains("not paired"), "{}", e);
        // a directory that names no source: neither the sibling rule nor
        // meta.json
        let d = scratch("nosrc");
        let dir = d.join("light").to_string_lossy().into_owned();
        let id = Identity {
            src_size: tree().len() as u64,
            src_mtime: 1,
        };
        write_files(&tree(), id, &dir, 1, DEFAULT_SPAN_BYTES).unwrap();
        let e = LightIndex::open(&dir).unwrap_err();
        assert!(e.contains("does not say which source"), "{}", e);
        // ... and the same directory with a meta.json naming the source
        let src = d.join("elsewhere.oas");
        std::fs::write(&src, tree()).unwrap();
        let id = file_identity(src.to_str().unwrap()).unwrap();
        write_files(&tree(), id, &dir, 1, DEFAULT_SPAN_BYTES).unwrap();
        std::fs::write(
            Path::new(&dir).join(META_FILE),
            format!("{{\n\"cache\": \"light\",\n\"src\": {{\"path\": \"{}\", \"size\": {}, \"mtime\": {}}}\n}}\n", src.display(), id.src_size, id.src_mtime),
        )
        .unwrap();
        let li = LightIndex::open(&dir).unwrap();
        assert_eq!(li.source, src.to_string_lossy());
        assert_eq!(meta_src_path("{\"src\": {\"path\": \"a\\\"b\\\\c\\n\"}}"), Some("a\"b\\c\n".to_string()));
    }
}
