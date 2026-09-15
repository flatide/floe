//! Bounded local diagnostics, not a wire dump. Unknown fields and invalid
//! numbers are excluded; future daemon text cannot leak through an allowlist.
use floe_worker_client::Frame;
use std::fmt::Write;

const METRICS: &[&str] = &[
    "plan_us",
    "text_plan_us",
    "read_us",
    "decode_us",
    "decode_sum_us",
    "decode_max_us",
    "index_us",
    "scene_us",
    "raster_us",
    "raster_wall_us",
    "raster_tile_max_us",
    "png_us",
    "publish_write_us",
    "publish_sync_us",
    "publish_rename_us",
    "workers",
    "decode_workers",
    "tiles",
    "tile_px",
    "pages",
    "cache_hit",
    "cache_miss",
    "cache_evict",
    "resident_bytes",
    "retained_bytes",
    "frame_cache_hit",
    "bin_items",
    "bin_overflow",
    "passes",
    "passes_skipped",
    "pass_workers",
    "batches",
    "summary_passes",
    "summary_cells",
    "summary_layers",
    "lod_swapped",
    "thin_pages",
    "rect_paints",
    "polygon_paints",
    "path_paints",
    "frame_paints",
    "hier_cells",
    "subtree_prunes",
];

pub(super) fn frame_line(pid: Option<u32>, frame: &Frame) -> String {
    let mut line = format!(
        "[render-perf] worker_pid={} gen={} round={} final={} partial={} deferred={} labels_truncated={} w={} h={} bytes={}",
        pid.unwrap_or(0), frame.generation, frame.round, u8::from(frame.final_frame),
        u8::from(frame.partial), frame.deferred, u8::from(frame.labels_truncated),
        frame.request.width, frame.request.height, frame.bytes.len(),
    );
    for key in METRICS {
        if let Ok(value) = frame.fields.u64(key) {
            // Canonical u64 formatting also bounds a zero-padded worker value.
            let _ = write!(line, " {key}={value}");
        }
    }
    line.push('\n');
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use floe_worker_client::{Fields, RenderRequest};
    use std::collections::BTreeMap;

    fn frame(fields: BTreeMap<String, String>) -> Frame {
        Frame {
            generation: 1,
            round: 2,
            final_frame: true,
            partial: false,
            deferred: 0,
            labels_truncated: true,
            request: RenderRequest::default(),
            bytes: b"not logged pixels".to_vec(),
            fields: Fields(fields),
        }
    }
    #[test]
    fn numeric_diagnostics_never_relay_text_paths_or_coordinates() {
        let mut f = frame(
            [
                ("plan_us", "0002"),
                ("raster_us", "9007199254740993"),
                ("pages", "/private/customer.oas"),
                ("cache_hit", "1\nsecret"),
                ("path", "/private/frame.raw"),
                ("cell", "PROPRIETARY"),
                ("future_number", "123456789"),
                ("png_us", "+1"),
                ("workers", "18446744073709551616"),
            ]
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect(),
        );
        f.request.view = [12345678., 0., 12345679., 1.];
        let line = frame_line(Some(42), &f);
        assert!(line.starts_with("[render-perf] worker_pid=42 gen=1 round=2 final=1 partial=0 deferred=0 labels_truncated=1 "));
        assert!(line.contains(" plan_us=2 raster_us=9007199254740993\n"));
        for private in [
            "private",
            "secret",
            "PROPRIETARY",
            "12345678",
            "future_number",
            "pixels",
            "png_us",
            "workers=",
        ] {
            assert!(!line.contains(private), "{line}");
        }
        assert_eq!(line.lines().count(), 1);
    }
    #[test]
    fn diagnostic_size_is_bounded_even_with_zero_padded_values() {
        let fields = METRICS
            .iter()
            .map(|k| ((*k).into(), u64::MAX.to_string()))
            .collect();
        assert!(frame_line(None, &frame(fields)).len() < 4096);
        let fields = [("pages".into(), "0".repeat(65536))].into();
        let line = frame_line(None, &frame(fields));
        assert!(line.ends_with(" pages=0\n"));
        assert!(line.len() < 512);
    }
}
