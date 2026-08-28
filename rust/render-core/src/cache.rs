use floe_oasis::doc::{Doc, Rep};
use floe_ovm::{BBox, PageV, CODEC_OASIS};
use floe_vfs::hier::HierPlan;
use floe_vfs::text::{LabelOpts, TextStats};
use floe_vfs::{Vfs, ViewReq};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use crate::font::label_planner_metrics;
use crate::{PlanRequest, RenderCancellation, RenderStats};

const MAX_DECODE_WORKERS: u16 = 256;

#[derive(Clone, Debug, PartialEq)]
pub struct CacheInfo {
    pub unit: f64,
    pub top_cell: u32,
    pub layers: u32,
    pub cells: u32,
    pub pages: u32,
    pub ovp_bytes: u64,
    /// Hierarchy height of the top cell — the deepest explicit depth
    /// level (matches the VFS daemon's max_depth).
    pub max_depth: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheLayer {
    pub index: u32,
    pub layer: u32,
    pub datatype: u32,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlanSummary {
    pub pages: u32,
    pub compressed_bytes: u64,
    pub encoded_bytes: u64,
    pub records: u64,
    pub members: u64,
    pub wc_cells: u64,
    pub wc_variants: u64,
    pub inst_edges: u64,
    pub frame_rects: u64,
}

pub struct PlannedView {
    pub plan: HierPlan,
    pub summary: PlanSummary,
    pub stats: RenderStats,
}

/// Display label selected by the parent VFS planner and resolved to the
/// renderer's stable OVM layer index. Block labels have no design layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderLabel {
    pub block: bool,
    pub white: bool,
    pub layer_idx: Option<u32>,
    pub x: i64,
    pub y: i64,
    pub rotation: u8,
    pub text: String,
}

pub struct PlannedLabels {
    pub rows: Vec<RenderLabel>,
    pub stats: TextStats,
    pub plan_us: u64,
}

#[derive(Debug)]
pub struct PagePayload {
    pub page_id: u32,
    pub meta: PageV,
    pub bytes: Vec<u8>,
}

pub struct DecodedPage {
    pub page_id: u32,
    pub layer_idx: u32,
    pub bbox: BBox,
    pub encoded_bytes: u32,
    pub records: u32,
    pub members: u64,
    pub doc: Doc,
    /// Record-extent index built once per decode and reused by every frame
    /// and raster tile that holds this page (F2R-03b).
    pub index: crate::PageIndex,
}

impl DecodedPage {
    /// Conservative charge used by the decoded-page LRU. This is an estimate,
    /// not allocator telemetry: shared repetition pools may be counted more
    /// than once, which is preferable to silently exceeding the budget.
    pub fn estimated_bytes(&self) -> u64 {
        let mut bytes = std::mem::size_of::<Self>() as u64;
        bytes = bytes.saturating_add(
            self.doc.cells.capacity() as u64 * std::mem::size_of::<floe_oasis::doc::Cell>() as u64,
        );
        bytes = bytes.saturating_add(
            self.doc.layer_order.capacity() as u64 * std::mem::size_of::<(u32, u32)>() as u64,
        );

        for cell in &self.doc.cells {
            bytes = bytes.saturating_add(cell.name.capacity() as u64);
            bytes = bytes.saturating_add(
                cell.rects.capacity() as u64
                    * std::mem::size_of::<floe_oasis::doc::RectRec>() as u64,
            );
            bytes = bytes.saturating_add(
                cell.polys.capacity() as u64
                    * std::mem::size_of::<floe_oasis::doc::PolyRec>() as u64,
            );
            bytes = bytes.saturating_add(
                cell.paths.capacity() as u64
                    * std::mem::size_of::<floe_oasis::doc::PathRec>() as u64,
            );
            bytes = bytes.saturating_add(
                cell.places.capacity() as u64
                    * std::mem::size_of::<floe_oasis::doc::PlaceRec>() as u64,
            );
            bytes = bytes.saturating_add(
                cell.texts.capacity() as u64
                    * std::mem::size_of::<floe_oasis::doc::TextRec>() as u64,
            );
            for rect in &cell.rects {
                bytes = bytes.saturating_add(rep_heap_bytes(&rect.rep));
            }
            for poly in &cell.polys {
                bytes = bytes.saturating_add(
                    poly.pts.capacity() as u64 * std::mem::size_of::<(i64, i64)>() as u64,
                );
                bytes = bytes.saturating_add(rep_heap_bytes(&poly.rep));
            }
            for path in &cell.paths {
                bytes = bytes.saturating_add(
                    path.pts.capacity() as u64 * std::mem::size_of::<(i64, i64)>() as u64,
                );
                bytes = bytes.saturating_add(rep_heap_bytes(&path.rep));
            }
            for place in &cell.places {
                bytes = bytes.saturating_add(rep_heap_bytes(&place.rep));
            }
            for text in &cell.texts {
                bytes = bytes.saturating_add(text.s.capacity() as u64);
                bytes = bytes.saturating_add(rep_heap_bytes(&text.rep));
            }
        }
        for name in self.doc.layer_names.values() {
            bytes = bytes.saturating_add(std::mem::size_of::<((u32, u32), String)>() as u64);
            bytes = bytes.saturating_add(name.capacity() as u64);
        }
        for aliases in self.doc.layer_aliases.values() {
            bytes = bytes.saturating_add(std::mem::size_of::<((u32, u32), Vec<String>)>() as u64);
            bytes = bytes
                .saturating_add(aliases.capacity() as u64 * std::mem::size_of::<String>() as u64);
            for alias in aliases {
                bytes = bytes.saturating_add(alias.capacity() as u64);
            }
        }
        bytes = bytes.saturating_add(self.index.estimated_bytes());
        bytes.max(self.encoded_bytes as u64)
    }
}

fn rep_heap_bytes(rep: &Rep) -> u64 {
    match rep {
        Rep::Pts(points) => points.len() as u64 * std::mem::size_of::<(i64, i64)>() as u64,
        Rep::One | Rep::Grid { .. } => 0,
    }
}

/// Read-only cache adapter. All parser and planner behavior comes from the
/// existing floe crates; only the currently-private OVP byte read is isolated
/// here.
pub struct Cache {
    vfs: Vfs,
}

impl Cache {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let dir = path
            .to_str()
            .ok_or_else(|| format!("cache path is not UTF-8: {}", path.display()))?;
        let vfs = Vfs::open(dir)?;
        Ok(Self { vfs })
    }

    pub fn info(&self) -> CacheInfo {
        CacheInfo {
            unit: self.vfs.ovm.unit,
            top_cell: self.vfs.ovm.top,
            layers: self.vfs.ovm.n_layers,
            cells: self.vfs.ovm.n_cells,
            pages: self.vfs.ovm.n_pages,
            ovp_bytes: self.vfs.ovm.ovp_len,
            // GUI depth cap; the same expression the VFS daemon
            // reports as max_depth (top cell hierarchy height)
            max_depth: if self.vfs.ovm.n_cells == 0 {
                0
            } else {
                self.vfs.ovm.cell(self.vfs.ovm.top).height
            },
        }
    }

    pub fn unit(&self) -> f64 {
        self.vfs.ovm.unit
    }

    /// Stable OVM layer-index mapping used by renderer paint-plane requests.
    pub fn layers(&self) -> Vec<CacheLayer> {
        (0..self.vfs.ovm.n_layers)
            .map(|index| {
                let layer = self.vfs.ovm.layer(index);
                CacheLayer {
                    index,
                    layer: layer.layer,
                    datatype: layer.dt,
                    name: layer.name,
                }
            })
            .collect()
    }

    /// Original design-cell name for query/pick provenance.
    pub fn cell_name(&self, cell_id: u32) -> Result<String, String> {
        if cell_id >= self.vfs.ovm.n_cells {
            return Err(format!(
                "cell index {} out of range 0..{}",
                cell_id, self.vfs.ovm.n_cells
            ));
        }
        Ok(self.vfs.ovm.cell(cell_id).name)
    }

    pub(crate) fn cell_bbox(&self, cell_id: u32) -> Result<BBox, String> {
        if cell_id >= self.vfs.ovm.n_cells {
            return Err(format!(
                "cell index {} out of range 0..{}",
                cell_id, self.vfs.ovm.n_cells
            ));
        }
        Ok(self.vfs.ovm.cell_rbbox(cell_id))
    }

    pub fn plan(&self, request: &PlanRequest) -> Result<PlannedView, String> {
        let req = self.view_request(request)?;
        let started = Instant::now();
        let plan = self.vfs.plan_hier(&req);
        let plan_us = elapsed_us(started);

        let mut summary = PlanSummary {
            pages: plan.pages.len().try_into().unwrap_or(u32::MAX),
            wc_cells: plan.stats.wc_cells,
            wc_variants: plan.stats.wc_variants,
            inst_edges: plan.stats.inst_edges,
            frame_rects: plan.stats.frame_rects,
            ..PlanSummary::default()
        };
        for &page_id in &plan.pages {
            let page = self.vfs.ovm.page(page_id);
            summary.compressed_bytes = summary
                .compressed_bytes
                .checked_add(page.csize as u64)
                .ok_or_else(|| "limit exceeded: selected compressed bytes".to_string())?;
            summary.encoded_bytes = summary
                .encoded_bytes
                .checked_add(page.usize_ as u64)
                .ok_or_else(|| "limit exceeded: selected encoded bytes".to_string())?;
            summary.records = summary
                .records
                .checked_add(page.records as u64)
                .ok_or_else(|| "limit exceeded: selected records".to_string())?;
            summary.members = summary
                .members
                .checked_add(page.members)
                .ok_or_else(|| "limit exceeded: selected members".to_string())?;
        }

        Ok(PlannedView {
            plan,
            summary,
            stats: RenderStats {
                plan_us,
                ..RenderStats::default()
            },
        })
    }

    /// Uses the same request-scoped VFS walk as the KLayout backend, without
    /// creating a TSV file or registering transient KLayout layers.
    pub fn plan_labels(
        &self,
        request: &PlanRequest,
        hierarchy_blocks: bool,
        font_px: f32,
    ) -> Result<PlannedLabels, String> {
        let req = self.view_request(request)?;
        let metrics = label_planner_metrics(font_px)?;
        let mut opts = LabelOpts {
            blocks: hierarchy_blocks,
            cell_px: metrics.declutter_cell_px,
            block_char_px: metrics.block_char_px,
            block_line_px: metrics.block_line_px,
            block_dots_px: metrics.block_dots_px,
            block_pad_px: metrics.block_pad_px,
            ..LabelOpts::default()
        };
        // Exact geometry renders are archival/probe operations. Their planner
        // scale is deliberately zero and therefore produces no display text.
        if request.exact {
            opts.blocks = false;
        }
        let started = Instant::now();
        let planned = self.vfs.plan_labels_with(&req, &opts)?;
        let plan_us = elapsed_us(started);
        let layer_indices: BTreeMap<(u32, u32), u32> = self
            .layers()
            .into_iter()
            .map(|layer| ((layer.layer, layer.datatype), layer.index))
            .collect();
        let mut rows = Vec::with_capacity(planned.rows.len());
        for row in planned.rows {
            let layer_idx = if row.block {
                None
            } else {
                Some(*layer_indices.get(&(row.layer, row.dt)).ok_or_else(|| {
                    format!("label references missing layer {}/{}", row.layer, row.dt)
                })?)
            };
            rows.push(RenderLabel {
                block: row.block,
                white: row.white,
                layer_idx,
                x: row.x,
                y: row.y,
                rotation: row.rot & 3,
                text: row.s,
            });
        }
        Ok(PlannedLabels {
            rows,
            stats: planned.stats,
            plan_us,
        })
    }

    fn view_request(&self, request: &PlanRequest) -> Result<ViewReq, String> {
        request.validate()?;
        Ok(ViewReq {
            view: request.view.as_bbox(),
            cut_dbu: if request.exact { 0 } else { request.cut_dbu },
            vis: self.vfs.layer_mask(request.visible_layers.as_deref())?,
            depth: request.depth,
            px_per_dbu: if request.exact {
                0.0
            } else {
                request.px_per_dbu
            },
        })
    }

    /// Reads pages through `floe-vfs`. The public contract preserves caller
    /// order and duplicates while performing the underlying IO in file order.
    pub fn read_pages(&self, page_ids: &[u32]) -> Result<Vec<PagePayload>, String> {
        let payloads = self.vfs.read_page_batch(page_ids)?;
        let mut pages = Vec::with_capacity(payloads.len());
        for (page_id, bytes) in payloads {
            let meta = self.vfs.ovm.page(page_id);
            if meta.codec != CODEC_OASIS {
                return Err(format!(
                    "unsupported page codec {} for page {}",
                    meta.codec, page_id
                ));
            }
            if bytes.len() != meta.csize as usize {
                return Err(format!(
                    "corrupt page {}: read {} bytes, expected {}",
                    page_id,
                    bytes.len(),
                    meta.csize
                ));
            }
            pages.push(PagePayload {
                page_id,
                meta,
                bytes,
            });
        }
        Ok(pages)
    }

    pub fn decode_pages(
        &self,
        page_ids: &[u32],
    ) -> Result<(Vec<DecodedPage>, RenderStats), String> {
        self.decode_pages_impl(page_ids, 1, None)
    }

    /// Reads the selected pages once in file order, then parses their OASIS
    /// payloads in parallel. Results and errors are resolved in caller order,
    /// independent of worker completion order.
    pub fn decode_pages_parallel(
        &self,
        page_ids: &[u32],
        workers: u16,
    ) -> Result<(Vec<DecodedPage>, RenderStats), String> {
        self.decode_pages_impl(page_ids, workers, None)
    }

    pub(crate) fn decode_pages_cancellable(
        &self,
        page_ids: &[u32],
        workers: u16,
        generation: u64,
        cancellation: &RenderCancellation,
    ) -> Result<(Vec<DecodedPage>, RenderStats), String> {
        self.decode_pages_impl(page_ids, workers, Some((generation, cancellation)))
    }

    fn decode_pages_impl(
        &self,
        page_ids: &[u32],
        workers: u16,
        guard: Option<(u64, &RenderCancellation)>,
    ) -> Result<(Vec<DecodedPage>, RenderStats), String> {
        if workers == 0 || workers > MAX_DECODE_WORKERS {
            return Err(format!(
                "decode workers must be in 1..={MAX_DECODE_WORKERS}: {workers}"
            ));
        }
        check_decode_cancelled(guard)?;
        let read_started = Instant::now();
        let payloads = self.read_pages(page_ids)?;
        let page_read_us = elapsed_us(read_started);
        check_decode_cancelled(guard)?;
        let decode_started = Instant::now();
        let worker_count = usize::from(workers).min(payloads.len());
        // Per-page wall time rides along with each output: the sum vs the
        // phase wall exposes worker idle time, the max exposes stragglers.
        let timed_decode = |payload: &PagePayload| {
            let page_started = Instant::now();
            let result = decode_payload(payload, guard);
            (result, elapsed_us(page_started))
        };
        let mut indexed = if worker_count <= 1 {
            let mut outputs = Vec::with_capacity(payloads.len());
            for (index, payload) in payloads.iter().enumerate() {
                check_decode_cancelled(guard)?;
                outputs.push((index, timed_decode(payload)));
            }
            outputs
        } else {
            let next_page = AtomicUsize::new(0);
            std::thread::scope(|scope| {
                let mut handles = Vec::with_capacity(worker_count);
                for _ in 0..worker_count {
                    let next_page = &next_page;
                    let payloads = &payloads;
                    let timed_decode = &timed_decode;
                    handles.push(scope.spawn(move || {
                        let mut outputs = Vec::new();
                        loop {
                            check_decode_cancelled(guard)?;
                            let index = next_page.fetch_add(1, Ordering::Relaxed);
                            let Some(payload) = payloads.get(index) else {
                                break;
                            };
                            outputs.push((index, timed_decode(payload)));
                        }
                        Ok::<_, String>(outputs)
                    }));
                }

                let mut outputs = Vec::with_capacity(payloads.len());
                let mut worker_error = None;
                for handle in handles {
                    match handle.join() {
                        Ok(Ok(mut worker_outputs)) => outputs.append(&mut worker_outputs),
                        Ok(Err(error)) => {
                            worker_error.get_or_insert(error);
                        }
                        Err(_) => {
                            worker_error
                                .get_or_insert_with(|| "page decode worker panicked".to_string());
                        }
                    }
                }
                if let Some(error) = worker_error {
                    return Err(error);
                }
                Ok(outputs)
            })?
        };
        check_decode_cancelled(guard)?;
        indexed.sort_unstable_by_key(|(index, _)| *index);
        if indexed.len() != payloads.len() {
            return Err(format!(
                "internal error: decoded {} of {} page payloads",
                indexed.len(),
                payloads.len()
            ));
        }

        let mut decoded = Vec::with_capacity(indexed.len());
        let mut decoded_bytes = 0u64;
        let mut page_decode_sum_us = 0u64;
        let mut page_decode_max_us = 0u64;
        let mut page_index_us = 0u64;
        for (expected_index, (index, (page, page_us))) in indexed.into_iter().enumerate() {
            if index != expected_index {
                return Err(format!(
                    "internal error: decoded page index {index}, expected {expected_index}"
                ));
            }
            let (page, index_us) = page?;
            page_decode_sum_us = page_decode_sum_us.saturating_add(page_us);
            page_decode_max_us = page_decode_max_us.max(page_us);
            page_index_us = page_index_us.saturating_add(index_us);
            decoded_bytes = decoded_bytes
                .checked_add(page.encoded_bytes as u64)
                .ok_or_else(|| "limit exceeded: decoded page bytes".to_string())?;
            decoded.push(page);
        }
        let page_decode_us = elapsed_us(decode_started);
        Ok((
            decoded,
            RenderStats {
                page_read_us,
                page_decode_us,
                page_decode_sum_us,
                page_decode_max_us,
                page_index_us,
                decode_workers_used: worker_count.try_into().unwrap_or(u16::MAX),
                decoded_cache_miss: page_ids.len().try_into().unwrap_or(u32::MAX),
                decoded_cache_bytes: decoded_bytes,
                ..RenderStats::default()
            },
        ))
    }
}

/// Decodes one page payload; the second value is the time spent
/// building the record index alone (the lazy-index decision input —
/// §3.11 measured it at +70% of raw decode on sample9).
fn decode_payload(
    payload: &PagePayload,
    guard: Option<(u64, &RenderCancellation)>,
) -> Result<(DecodedPage, u64), String> {
    let doc = floe_oasis::doc::parse_doc(&payload.bytes)
        .map_err(|error| format!("decode page {}: {error}", payload.page_id))?;
    if doc.cells.len() != 1 {
        return Err(format!(
            "corrupt page {}: expected one cell, decoded {}",
            payload.page_id,
            doc.cells.len()
        ));
    }
    // The build probes the guard every few thousand records so a
    // stale generation stops burning CPU inside a large page.
    let index_started = Instant::now();
    let index = crate::PageIndex::build_cancellable(&doc, &mut || {
        check_decode_cancelled(guard)
    })?;
    let index_us = elapsed_us(index_started);
    Ok((
        DecodedPage {
            page_id: payload.page_id,
            layer_idx: payload.meta.layer_idx,
            bbox: payload.meta.bbox,
            encoded_bytes: payload.meta.usize_,
            records: payload.meta.records,
            members: payload.meta.members,
            doc,
            index,
        },
        index_us,
    ))
}

fn check_decode_cancelled(guard: Option<(u64, &RenderCancellation)>) -> Result<(), String> {
    if let Some((generation, cancellation)) = guard {
        cancellation.check(generation)?;
    }
    Ok(())
}

fn elapsed_us(started: Instant) -> u64 {
    started.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use floe_oasis::write::W;

    #[test]
    fn corrupt_page_repetition_count_returns_a_decode_error() {
        let mut w = W::new();
        w.out.extend_from_slice(b"%SEMI-OASIS\r\n");
        w.uint(1);
        w.string(b"1.0");
        w.real_f64(1000.0);
        w.uint(0);
        for _ in 0..12 {
            w.uint(0);
        }
        w.uint(14);
        w.string(b"TOP");
        w.uint(20);
        w.byte(0x7f); // layer, datatype, width, height, x, y, repetition
        w.uint(1);
        w.uint(0);
        w.uint(10);
        w.uint(10);
        w.sint(0);
        w.sint(0);
        w.uint(4);
        w.uint(1 << 40);

        let payload = PagePayload {
            page_id: 77,
            meta: PageV {
                cell: 0,
                layer_idx: 0,
                seq: 0,
                lod: 0,
                codec: CODEC_OASIS,
                bbox: BBox {
                    x0: 0,
                    y0: 0,
                    x1: 10,
                    y1: 10,
                },
                file_off: 0,
                csize: w.out.len() as u32,
                usize_: w.out.len() as u32,
                records: 1,
                lod_page: u32::MAX,
                members: 1,
                max_w: 10,
                max_h: 10,
                max_min: 10,
            },
            bytes: w.out,
        };
        let error = match decode_payload(&payload, None) {
            Ok(_) => panic!("corrupt page decoded successfully"),
            Err(error) => error,
        };
        assert!(error.contains("decode page 77"), "{error}");
        assert!(
            error.contains("repetition x offsets count") && error.contains("remaining bytes"),
            "{error}"
        );
    }
}
