//! Strict browser DTOs and a self-contained frame envelope. Paths/argv/native
//! commands never enter these APIs. World coordinates and u64 IDs are strings.
use floe_app_core::{
    shots::{Detail, Thin},
    view::{Depth, DisplayFrame, Model, Navigation, Patch, Phase, Snapshot},
};
use floe_worker_client::{Fill, FrameFormat, Layers, Style};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};

pub const HEADER_BYTES: usize = 64 * 1024;
pub const PAYLOAD_BYTES: usize = 80 * 1024 * 1024;
pub const PACKET_BYTES: usize = 4 + HEADER_BYTES + PAYLOAD_BYTES;
pub const CONTROL_REPLY_BYTES: usize = 256 * 1024;
pub const OUTPUT_BUDGET: usize = 256 * 1024 * 1024;

pub fn counter(s: &str) -> Result<u64, &'static str> {
    let n = s.parse::<u64>().map_err(|_| "invalid counter")?;
    if n == 0 || s != n.to_string() {
        return Err("invalid counter");
    }
    Ok(n)
}
fn decimal(s: &str) -> Result<f64, &'static str> {
    if s.len() > 64 || s.trim() != s {
        return Err("invalid coordinate");
    }
    let n = s.parse::<f64>().map_err(|_| "invalid coordinate")?;
    if !n.is_finite() {
        return Err("invalid coordinate");
    }
    Ok(n)
}
/// serde Option silently accepts JSON null. A patch distinguishes omitted
/// from present: null is rejected unless it is a valid value of T (none here).
#[derive(Debug, Default)]
pub enum Field<T> {
    #[default]
    Absent,
    Value(T),
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Field<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        T::deserialize(d).map(Self::Value)
    }
}
impl<T> Field<T> {
    fn optional(self) -> Option<T> {
        match self {
            Self::Absent => None,
            Self::Value(v) => Some(v),
        }
    }
}
#[derive(Deserialize, Debug)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Nav {
    Fit {},
    Goto {
        center_um: [String; 2],
        width_um: String,
    },
    Pan {
        x: f64,
        y: f64,
        snap: bool,
    },
    Zoom {
        factor: f64,
        anchor: [f64; 2],
    },
}
impl Nav {
    fn core(self) -> Result<Navigation, &'static str> {
        Ok(match self {
            Self::Fit {} => Navigation::Fit,
            Self::Goto {
                center_um,
                width_um,
            } => Navigation::Goto {
                center_um: [decimal(&center_um[0])?, decimal(&center_um[1])?],
                width_um: decimal(&width_um)?,
            },
            Self::Pan { x, y, snap } => Navigation::Pan { x, y, snap },
            Self::Zoom { factor, anchor } => Navigation::Zoom { factor, anchor },
        })
    }
}
#[derive(Deserialize, Debug)]
#[serde(try_from = "String")]
pub enum DetailDto {
    Exact,
    Low,
    Medium,
    High,
}
impl TryFrom<String> for DetailDto {
    type Error = &'static str;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        match s.as_str() {
            "exact" => Ok(Self::Exact),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            _ => Err("invalid detail"),
        }
    }
}
#[derive(Deserialize, Debug)]
#[serde(try_from = "String")]
pub enum ThinDto {
    Auto,
    Keep,
    Cull,
}
impl TryFrom<String> for ThinDto {
    type Error = &'static str;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        match s.as_str() {
            "auto" => Ok(Self::Auto),
            "keep" => Ok(Self::Keep),
            "cull" => Ok(Self::Cull),
            _ => Err("invalid thin policy"),
        }
    }
}
#[derive(Deserialize, Debug)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Selection {
    All {},
    None {},
    Only { pairs: Vec<(u32, u32)> },
}
#[derive(Deserialize, Debug)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FillDto {
    Solid {},
    Clear {},
    Speckle {},
    Pattern { rows: [u16; 16] },
}
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct StyleDto {
    pub pair: (u32, u32),
    pub color: String,
    pub fill: FillDto,
    pub width: u8,
}
impl StyleDto {
    fn core(self) -> Result<Style, &'static str> {
        // Exactly #RRGGBB, not a file/CSS expression or arbitrary alpha.
        if self.color.len() != 7 || !self.color.is_ascii() || !self.color.starts_with('#') {
            return Err("invalid style color");
        }
        let color = floe_app_core::styles::color(&self.color).ok_or("invalid style color")?;
        let fill = match self.fill {
            FillDto::Solid {} => Fill::Solid,
            FillDto::Clear {} => Fill::Clear,
            FillDto::Speckle {} => Fill::Speckle,
            FillDto::Pattern { rows } => Fill::Pattern(rows),
        };
        Ok(Style {
            layer: self.pair,
            color,
            fill,
            width: self.width,
        })
    }
}
#[derive(Default, Deserialize, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct PatchDto {
    pub navigation: Field<Nav>,
    pub pixels: Field<(u32, u32)>,
    pub depth: Field<String>,
    pub detail: Field<DetailDto>,
    pub thin: Field<ThinDto>,
    pub layers: Field<Selection>,
    pub frames: Field<bool>,
    pub labels: Field<bool>,
    pub font_px: Field<u32>,
    pub mono: Field<bool>,
    pub styles: Field<Vec<StyleDto>>,
}
impl PatchDto {
    pub fn core(self) -> Result<Patch, &'static str> {
        let depth = self
            .depth
            .optional()
            .map(|s| {
                if s == "full" {
                    Ok(Depth::Full)
                } else {
                    let n = s.parse::<u32>().map_err(|_| "invalid depth")?;
                    if n.to_string() != s {
                        return Err("invalid depth");
                    }
                    Ok(Depth::Levels(n))
                }
            })
            .transpose()?;
        let style_changes = self
            .styles
            .optional()
            .unwrap_or_default()
            .into_iter()
            .map(StyleDto::core)
            .collect::<Result<_, _>>()?;
        Ok(Patch {
            navigation: self.navigation.optional().map(Nav::core).transpose()?,
            pixels: self.pixels.optional(),
            depth,
            detail: self.detail.optional().map(|d| match d {
                DetailDto::Exact => Detail::Exact,
                DetailDto::Low => Detail::Low,
                DetailDto::Medium => Detail::Medium,
                DetailDto::High => Detail::High,
            }),
            thin: self.thin.optional().map(|t| match t {
                ThinDto::Auto => Thin::Auto,
                ThinDto::Keep => Thin::Keep,
                ThinDto::Cull => Thin::Cull,
            }),
            layers: self.layers.optional().map(|l| match l {
                Selection::All {} => Layers::All,
                Selection::None {} => Layers::None,
                Selection::Only { pairs } => Layers::Only(pairs),
            }),
            frames: self.frames.optional(),
            labels: self.labels.optional(),
            font_px: self.font_px.optional(),
            mono: self.mono.optional(),
            style_changes,
        })
    }
}
fn phase(p: Phase) -> &'static str {
    match p {
        Phase::Opening => "opening",
        Phase::Rendering => "rendering",
        Phase::Cancelling => "cancelling",
        Phase::Idle => "idle",
        Phase::Closed => "closed",
        Phase::Failed => "failed",
    }
}
pub fn snapshot(s: &Snapshot, m: &Model, view_id: &str, connection_epoch: &str) -> Value {
    let v = &s.state;
    let layers = match &v.layers {
        Layers::All => json!({"mode":"all"}),
        Layers::None => json!({"mode":"none"}),
        Layers::Only(pairs) => json!({"mode":"only","pairs":pairs}),
    };
    json!({"type":"snapshot","view_id":view_id,"connection_epoch":connection_epoch,"dataset_revision":m.dataset_revision.to_string(),
        "state_rev":s.state_rev.to_string(),"render_rev":s.render_rev.to_string(),"render_key":s.render_key.to_string(),"worker_epoch":s.worker_epoch.to_string(),
        "bbox_dbu":v.viewport.bbox.map(|n|n.to_string()),"dbu_um":m.dbu.to_string(),"pixels":[v.viewport.width,v.viewport.height],
        "depth":v.depth.map_or("full".into(),|n|n.to_string()),"max_depth":s.max_depth.map(|n|n.to_string()),
        "detail":match v.detail {Detail::Exact=>"exact",Detail::Low=>"low",Detail::Medium=>"medium",Detail::High=>"high"},
        "thin":v.thin.name(),"effective_thin":match v.thin.effective(m.deck){floe_worker_client::ThinPolicy::Keep=>"keep",_=>"cull"},
        "layers":layers,"frames":v.frames,"labels":v.labels,"font_px":v.font_px,"mono":v.mono,
        "status":phase(s.phase),"source_stale":m.source_stale,"deck_skipped":m.skipped.to_string(),
        "failure":s.failure.as_ref().map(|(kind,_)|safe_error(*kind)),
        "submitted":s.submitted.to_string(),"consumed":s.consumed.to_string(),"discarded":s.discarded.to_string(),
        "capabilities":{"labels":!m.deck,"frames":true,"margin":false,"query":false,"clip":false,"edit_source":false}})
}
pub fn safe_error(kind: floe_app_core::ErrorKind) -> &'static str {
    use floe_app_core::ErrorKind as K;
    match kind {
        K::InvalidInput => "invalid_request",
        K::Unsupported => "unsupported",
        K::Io => "io_error",
        K::Cache => "index_unavailable",
        K::Busy => "busy",
        K::Version => "worker_version",
        K::Worker => "worker_failed",
        K::Cancelled => "cancelled",
        K::Incomplete => "incomplete",
    }
}
/// The allowlist contains numeric telemetry only. No future worker diagnostic
/// or path field can accidentally appear in an authenticated shared frame.
const PERF: &[&str] = &[
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
    "summary_passes",
    "summary_cells",
    "summary_layers",
    "lod_swapped",
    "thin_pages",
    "rect_paints",
    "polygon_paints",
    "path_paints",
    "frame_paints",
    "labels",
    "passes",
    "passes_skipped",
    "pass_workers",
    "batches",
    "frame_cache_hit",
    "bin_items",
    "bin_overflow",
    "hier_cells",
    "subtree_prunes",
];
pub fn frame_header(
    frame: &DisplayFrame,
    view_id: &str,
    epoch: &str,
) -> Result<Vec<u8>, &'static str> {
    let f = &frame.frame;
    let r = &f.request;
    if f.bytes.len() > PAYLOAD_BYTES
        || view_id.len() > 128
        || epoch.len() > 128
        || r.width == 0
        || r.height == 0
        || r.width > 8192
        || r.height > 8192
        || !r.view.iter().all(|n| n.is_finite())
        || r.view[0] >= r.view[2]
        || r.view[1] >= r.view[3]
        || u64::from(r.width) * u64::from(r.height) > 16 * 1024 * 1024
    {
        return Err("frame limit");
    }
    match r.format {
        FrameFormat::Raw => {
            let count = 16u64 + 4 * u64::from(r.width) * u64::from(r.height);
            if f.bytes.len() as u64 != count
                || !f.bytes.starts_with(b"FLOERAW1")
                || f.bytes[8..12] != r.width.to_le_bytes()
                || f.bytes[12..16] != r.height.to_le_bytes()
            {
                return Err("raw frame mismatch");
            }
        }
        FrameFormat::Png => {
            // Full PNG framing/CRC is already checked by WorkerClient.
            if f.bytes.len() < 33
                || !f.bytes.starts_with(b"\x89PNG\r\n\x1a\n")
                || &f.bytes[12..16] != b"IHDR"
                || f.bytes[16..20] != r.width.to_be_bytes()
                || f.bytes[20..24] != r.height.to_be_bytes()
            {
                return Err("PNG frame mismatch");
            }
        }
    }
    let perf: serde_json::Map<String, Value> = PERF
        .iter()
        .filter_map(|k| {
            let v = f.fields.get(k)?;
            v.parse::<u64>().ok()?;
            Some(((*k).into(), json!(v)))
        })
        .collect();
    let approximate = [
        "summary_passes",
        "summary_cells",
        "summary_layers",
        "lod_swapped",
        "wide_washes",
        "washed",
    ]
    .iter()
    .any(|k| {
        f.fields
            .get(k)
            .and_then(|v| v.parse::<u64>().ok())
            .is_some_and(|v| v > 0)
    });
    let value = json!({"type":"frame","protocol":1,"view_id":view_id,"connection_epoch":epoch,"frame_id":frame.id.to_string(),
        "dataset_revision":frame.dataset_revision.to_string(),"state_rev":frame.state_rev.to_string(),"render_rev":frame.render_rev.to_string(),
        "render_key":frame.render_key.to_string(),"worker_epoch":frame.worker_epoch.to_string(),"generation":f.generation.to_string(),"round":f.round.to_string(),
        "purpose":"foreground","bbox_dbu":r.view.map(|n|n.to_string()),"width":r.width,"height":r.height,"row0":"top",
        "format":match r.format {FrameFormat::Raw=>"raw",FrameFormat::Png=>"png"},"payload_length":f.bytes.len().to_string(),
        "final":f.final_frame,"partial":f.partial,"deferred":f.deferred.to_string(),"labels_truncated":f.labels_truncated,
        "deck_skipped":frame.deck_skipped.to_string(),"complete":f.complete()&&frame.deck_skipped==0,"approximate":approximate,"query":false,"perf":perf});
    let header = serde_json::to_vec(&value).map_err(|_| "frame metadata encoding failed")?;
    if header.len() > HEADER_BYTES {
        return Err("frame metadata limit");
    }
    Ok(header)
}
pub fn packet(header: &[u8], payload: &[u8]) -> Result<Vec<u8>, &'static str> {
    if header.len() > HEADER_BYTES || payload.len() > PAYLOAD_BYTES {
        return Err("frame limit");
    }
    let mut out = Vec::with_capacity(4 + header.len() + payload.len());
    out.extend((header.len() as u32).to_le_bytes());
    out.extend(header);
    out.extend(payload);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use floe_worker_client::{Fields, Frame, RenderRequest};
    use std::collections::BTreeMap;
    #[test]
    fn null_unknown_fields_duplicate_fields_and_numeric_world_coordinates_are_not_patches() {
        for text in [
            r#"{"thin":null}"#,
            r#"{"thin":{"keep":null}}"#,
            r#"{"detail":{"high":null}}"#,
            r#"{"labels":null}"#,
            r#"{"pixels":null}"#,
            r#"{"styles":null}"#,
            r#"{"depth":null}"#,
            r#"{"out":"/tmp/a"}"#,
            r#"{"thin":"cull","thin":"keep"}"#,
            r#"{"navigation":{"kind":"goto","center_um":[1,2],"width_um":"5"}}"#,
            r#"{"navigation":{"kind":"fit","extra":1}}"#,
            r#"{"layers":{"mode":"all","pairs":[]}}"#,
            r##"{"styles":[{"pair":[1,0],"color":"#00ff00","fill":{"kind":"solid","extra":1},"width":1}]}"##,
        ] {
            assert!(serde_json::from_str::<PatchDto>(text).is_err(), "{text}");
        }
        assert!(serde_json::from_str::<PatchDto>("{}")
            .unwrap()
            .core()
            .is_ok());
        for text in [
            r#"{"depth":"01"}"#,
            r#"{"depth":"-1"}"#,
            r#"{"navigation":{"kind":"goto","center_um":["NaN","0"],"width_um":"5"}}"#,
            r#"{"styles":[{"pair":[1,0],"color":"한글","fill":{"kind":"solid"},"width":1}]}"#,
        ] {
            assert!(
                serde_json::from_str::<PatchDto>(text)
                    .unwrap()
                    .core()
                    .is_err(),
                "{text}"
            );
        }
        for value in ["0", "01", "+1", "-1", "18446744073709551616", "1.0"] {
            assert!(counter(value).is_err());
        }
        assert_eq!(counter("18446744073709551615").unwrap(), u64::MAX);
    }
    fn frame() -> DisplayFrame {
        let mut bytes = b"FLOERAW1".to_vec();
        bytes.extend(3u32.to_le_bytes());
        bytes.extend(2u32.to_le_bytes());
        bytes.resize(40, 255);
        DisplayFrame {
            id: u64::MAX,
            dataset_revision: u64::MAX - 1,
            state_rev: 7,
            render_rev: 6,
            render_key: 2,
            worker_epoch: 8,
            deck_skipped: 0,
            frame: Frame {
                generation: 4,
                round: 3,
                final_frame: true,
                partial: false,
                deferred: 0,
                labels_truncated: false,
                request: RenderRequest {
                    view: [-10.9375, 0., 23.125, 9.75],
                    width: 3,
                    height: 2,
                    format: FrameFormat::Raw,
                    ..Default::default()
                },
                bytes,
                fields: Fields(BTreeMap::from([
                    ("png".into(), "/secret/path".into()),
                    ("raster_us".into(), "9007199254740993".into()),
                    ("decode_us".into(), "NaN".into()),
                    ("summary_cells".into(), "15".into()),
                ])),
            },
        }
    }
    #[test]
    fn frame_envelope_is_atomic_exact_and_redacts_unlisted_native_fields() {
        let mut f = frame();
        let header = frame_header(&f, "view", "connection").unwrap();
        let h: Value = serde_json::from_slice(&header).unwrap();
        assert_eq!(h["frame_id"], u64::MAX.to_string());
        assert_eq!(h["bbox_dbu"][0], "-10.9375");
        assert_eq!(h["perf"]["raster_us"], "9007199254740993");
        assert!(h["perf"].get("png").is_none());
        assert!(h["perf"].get("decode_us").is_none());
        assert!(!String::from_utf8(header.clone())
            .unwrap()
            .contains("/secret"));
        assert_eq!(h["complete"], true);
        assert_eq!(h["approximate"], true);
        assert_eq!(h["query"], false);
        let out = packet(&header, &f.frame.bytes).unwrap();
        let n = u32::from_le_bytes(out[..4].try_into().unwrap()) as usize;
        assert_eq!(&out[4..4 + n], &header);
        assert_eq!(&out[4 + n..], &f.frame.bytes);
        f.frame.partial = true;
        let h: Value = serde_json::from_slice(&frame_header(&f, "v", "c").unwrap()).unwrap();
        assert_eq!(h["complete"], false);
        f.frame.partial = false;
        f.deck_skipped = 1;
        let h: Value = serde_json::from_slice(&frame_header(&f, "v", "c").unwrap()).unwrap();
        assert_eq!(h["complete"], false);
    }
    #[test]
    fn malformed_frame_dimensions_payload_and_header_size_fail_before_encoding() {
        let mut f = frame();
        f.frame.bytes.pop();
        assert!(frame_header(&f, "v", "c").is_err());
        let mut f = frame();
        f.frame.request.width = 0;
        assert!(frame_header(&f, "v", "c").is_err());
        let mut f = frame();
        f.frame.request.view[0] = f64::NAN;
        assert!(frame_header(&f, "v", "c").is_err());
        assert!(frame_header(&frame(), &"v".repeat(129), "c").is_err());
        assert!(packet(&vec![0; HEADER_BYTES + 1], &[]).is_err());
    }
}
