//! Platform-independent label font rasterization.
//!
//! The font bytes are part of the renderer, so output never depends on the
//! host's font catalog, DPI settings, FreeType build, or KLayout text engine.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock};

use fontdue::{Font, FontSettings};

use crate::RenderLabel;

pub const DEFAULT_LABEL_FONT_PX: f32 = 14.0;
pub const MIN_LABEL_FONT_PX: f32 = 6.0;
pub const MAX_LABEL_FONT_PX: f32 = 96.0;

// These values are the display-policy calibration for the bundled font at
// DEFAULT_LABEL_FONT_PX.  The VFS text walk uses them before any glyph bitmap
// exists, so they must scale with the requested font size as one contract.
const DEFAULT_DECLUTTER_CELL_PX: f64 = 48.0;
const DEFAULT_BLOCK_CHAR_PX: f64 = 8.0;
const DEFAULT_BLOCK_LINE_PX: f64 = 14.0;
const DEFAULT_BLOCK_DOTS_PX: f64 = 3.0;
const DEFAULT_BLOCK_PAD_PX: f64 = 4.0;

const FONT_BYTES: &[u8] = include_bytes!("../assets/NotoSansMono-Regular.ttf");

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LabelPlannerMetrics {
    pub declutter_cell_px: f64,
    pub block_char_px: f64,
    pub block_line_px: f64,
    pub block_dots_px: f64,
    pub block_pad_px: f64,
}

pub(crate) fn label_planner_metrics(font_px: f32) -> Result<LabelPlannerMetrics, String> {
    validate_font_px(font_px)?;
    let scale = f64::from(font_px / DEFAULT_LABEL_FONT_PX);
    Ok(LabelPlannerMetrics {
        declutter_cell_px: DEFAULT_DECLUTTER_CELL_PX * scale,
        block_char_px: DEFAULT_BLOCK_CHAR_PX * scale,
        block_line_px: DEFAULT_BLOCK_LINE_PX * scale,
        block_dots_px: DEFAULT_BLOCK_DOTS_PX * scale,
        block_pad_px: DEFAULT_BLOCK_PAD_PX * scale,
    })
}

#[derive(Debug)]
pub(crate) struct RasterGlyph {
    pub xmin: i32,
    pub ymin: i32,
    pub width: usize,
    pub height: usize,
    pub advance: f32,
    pub alpha: Vec<u8>,
}

#[derive(Debug)]
pub(crate) struct GlyphAtlas {
    pub ascent: f32,
    pub descent: f32,
    glyphs: BTreeMap<char, Arc<RasterGlyph>>,
    kern: BTreeMap<(char, char), f32>,
}

impl GlyphAtlas {
    pub fn build(labels: &[RenderLabel], font_px: f32) -> Result<Self, String> {
        validate_font_px(font_px)?;
        let font = bundled_font()?;
        let line = font
            .horizontal_line_metrics(font_px)
            .ok_or_else(|| "bundled label font has no horizontal metrics".to_string())?;
        let mut chars = BTreeSet::new();
        let mut pairs = BTreeSet::new();
        for label in labels {
            let mut previous = None;
            for ch in normalized_chars(&label.text) {
                chars.insert(ch);
                if let Some(left) = previous {
                    pairs.insert((left, ch));
                }
                previous = Some(ch);
            }
        }
        let mut glyphs = BTreeMap::new();
        for ch in chars {
            glyphs.insert(ch, cached_glyph(font, ch, font_px)?);
        }
        let kern = pairs
            .into_iter()
            .filter_map(|pair| {
                font.horizontal_kern(pair.0, pair.1, font_px)
                    .filter(|value| *value != 0.0)
                    .map(|value| (pair, value))
            })
            .collect();
        Ok(Self {
            ascent: line.ascent,
            descent: line.descent,
            glyphs,
            kern,
        })
    }

    pub fn glyph(&self, ch: char) -> &RasterGlyph {
        self.glyphs[&normalize_char(ch)].as_ref()
    }

    pub fn kern(&self, left: char, right: char) -> f32 {
        self.kern
            .get(&(normalize_char(left), normalize_char(right)))
            .copied()
            .unwrap_or(0.0)
    }
}

pub(crate) fn normalized_chars(text: &str) -> impl Iterator<Item = char> + '_ {
    text.chars().map(normalize_char)
}

fn normalize_char(ch: char) -> char {
    match ch {
        '\n' | '\r' | '\t' | '\0' => ' ',
        value => value,
    }
}

pub fn validate_font_px(font_px: f32) -> Result<(), String> {
    if !font_px.is_finite()
        || font_px.fract() != 0.0
        || !(MIN_LABEL_FONT_PX..=MAX_LABEL_FONT_PX).contains(&font_px)
    {
        return Err(format!(
            "label font size must be a whole pixel in {MIN_LABEL_FONT_PX}..={MAX_LABEL_FONT_PX}px, got {font_px}"
        ));
    }
    Ok(())
}

fn cached_glyph(font: &Font, ch: char, font_px: f32) -> Result<Arc<RasterGlyph>, String> {
    // EDA labels are overwhelmingly ASCII. Cache that bounded set across
    // progressive rounds and generations; uncommon Unicode remains
    // request-local so arbitrary input cannot grow a process-global cache.
    if !ch.is_ascii() {
        return raster_glyph(font, ch, font_px).map(Arc::new);
    }
    type Key = (u32, char);
    static ASCII_GLYPHS: OnceLock<Mutex<BTreeMap<Key, Arc<RasterGlyph>>>> = OnceLock::new();
    let cache = ASCII_GLYPHS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let key = (font_px.to_bits(), ch);
    let mut cache = cache
        .lock()
        .map_err(|_| "bundled label glyph cache is poisoned".to_string())?;
    if let Some(glyph) = cache.get(&key) {
        return Ok(Arc::clone(glyph));
    }
    let glyph = Arc::new(raster_glyph(font, ch, font_px)?);
    cache.insert(key, Arc::clone(&glyph));
    Ok(glyph)
}

fn raster_glyph(font: &Font, ch: char, font_px: f32) -> Result<RasterGlyph, String> {
    let (metrics, alpha) = font.rasterize(ch, font_px);
    let expected = metrics
        .width
        .checked_mul(metrics.height)
        .ok_or_else(|| "label glyph bitmap size overflow".to_string())?;
    if alpha.len() != expected {
        return Err(format!(
            "bundled label font returned {} pixels for {}x{} glyph",
            alpha.len(),
            metrics.width,
            metrics.height
        ));
    }
    Ok(RasterGlyph {
        xmin: metrics.xmin,
        ymin: metrics.ymin,
        width: metrics.width,
        height: metrics.height,
        advance: metrics.advance_width,
        alpha,
    })
}

fn bundled_font() -> Result<&'static Font, String> {
    static FONT: OnceLock<Result<Font, String>> = OnceLock::new();
    FONT.get_or_init(|| {
        Font::from_bytes(FONT_BYTES, FontSettings::default())
            .map_err(|error| format!("load bundled label font: {error}"))
    })
    .as_ref()
    .map_err(Clone::clone)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(text: &str) -> RenderLabel {
        RenderLabel {
            block: false,
            white: false,
            layer_idx: Some(0),
            x: 0,
            y: 0,
            rotation: 0,
            text: text.to_string(),
        }
    }

    #[test]
    fn bundled_font_is_available_without_host_lookup() {
        let atlas = GlyphAtlas::build(&[label("ABC xyz 123")], DEFAULT_LABEL_FONT_PX).unwrap();
        assert!(atlas.ascent > 0.0);
        assert!(atlas.descent < 0.0);
        assert!(atlas.glyph('A').alpha.iter().any(|value| *value != 0));
        assert!(atlas.glyph('x').advance > 0.0);
    }

    #[test]
    fn control_characters_use_a_bounded_space_glyph() {
        let atlas = GlyphAtlas::build(&[label("A\tB\nC")], DEFAULT_LABEL_FONT_PX).unwrap();
        assert_eq!(atlas.glyph('\t').advance, atlas.glyph(' ').advance);
        assert_eq!(atlas.glyph('\n').alpha, atlas.glyph(' ').alpha);
    }

    #[test]
    fn rejects_unbounded_font_sizes() {
        assert!(validate_font_px(0.0).is_err());
        assert!(validate_font_px(f32::NAN).is_err());
        assert!(validate_font_px(MAX_LABEL_FONT_PX + 1.0).is_err());
        assert!(validate_font_px(14.5).is_err());
    }

    #[test]
    fn planner_spacing_scales_with_the_raster_font() {
        let default = label_planner_metrics(DEFAULT_LABEL_FONT_PX).unwrap();
        assert_eq!(default.declutter_cell_px, 48.0);
        assert_eq!(default.block_char_px, 8.0);
        assert_eq!(default.block_line_px, 14.0);
        assert_eq!(default.block_dots_px, 3.0);
        assert_eq!(default.block_pad_px, 4.0);

        let double = label_planner_metrics(DEFAULT_LABEL_FONT_PX * 2.0).unwrap();
        assert_eq!(double.declutter_cell_px, 96.0);
        assert_eq!(double.block_char_px, 16.0);
        assert_eq!(double.block_line_px, 28.0);
        assert_eq!(double.block_dots_px, 6.0);
        assert_eq!(double.block_pad_px, 8.0);
    }

    #[test]
    fn planner_spacing_rejects_a_size_the_rasterizer_rejects() {
        assert!(label_planner_metrics(MIN_LABEL_FONT_PX - 1.0).is_err());
        assert!(label_planner_metrics(14.5).is_err());
    }
}
