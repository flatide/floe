use floe_render_core::{
    render_geometry_occupancy, render_geometry_styled, Cache, CacheLayer, DecodedPageCache,
    FrameScene, GeometryRasterRequest, LayerFill, LayerStyle, PlanRequest, RasterViewBox,
    StyledGeometryRasterRequest, ViewBox, DEFAULT_TILE_SIZE, MAX_TILE_SIZE,
};
use std::env;
use std::process::ExitCode;

const USAGE: &str = "usage: floe-render-cli CACHE --view x0,y0,x1,y1 \
    --width PX --height PX [--depth full|N] [--cut-px N] \
    [--layers L/D,...] [--decode-pages N] [--budget-mb N] [--jobs N] [--tile-px N] \
    [--style 'L/D,#RRGGBB,solid|speckle|clear|pat:HEX64[,1..8]'] \
    [--frames on|off] [--mono on|off] \
    [--out FILE]\n\
    --view coordinates are microns; --out writes geometry-fill occupancy PNG";
const MAX_JOBS: u16 = 256;

#[derive(Debug)]
struct Args {
    cache: String,
    view_um: [f64; 4],
    width: u32,
    height: u32,
    depth: u32,
    cut_px: f64,
    layers: Option<Vec<String>>,
    decode_pages: usize,
    budget_mb: u64,
    jobs: u16,
    tile_size: u16,
    styles: Vec<String>,
    frames: bool,
    mono: bool,
    out: Option<String>,
}

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn run(raw: Vec<String>) -> Result<(), String> {
    let args = parse_args(&raw)?;
    let cache = Cache::open(&args.cache)?;
    let info = cache.info();
    let request = make_request(&args, info.unit)?;
    let planned = cache.plan(&request)?;

    println!(
        "cache\tunit={}\ttop={}\tlayers={}\tcells={}\tpages={}\tovp_bytes={}",
        info.unit, info.top_cell, info.layers, info.cells, info.pages, info.ovp_bytes
    );
    println!(
        "plan\tpages={}\tcompressed_bytes={}\tencoded_bytes={}\trecords={}\tmembers={}\twc_cells={}\twc_variants={}\tinst_edges={}\tframe_rects={}\tplan_us={}",
        planned.summary.pages,
        planned.summary.compressed_bytes,
        planned.summary.encoded_bytes,
        planned.summary.records,
        planned.summary.members,
        planned.summary.wc_cells,
        planned.summary.wc_variants,
        planned.summary.inst_edges,
        planned.summary.frame_rects,
        planned.stats.plan_us,
    );

    let mut prioritized: Vec<(u64, u32)> = planned
        .plan
        .page_prio
        .iter()
        .copied()
        .zip(planned.plan.pages.iter().copied())
        .collect();
    prioritized.sort_unstable();
    let selected: Vec<u32> = prioritized
        .into_iter()
        .take(args.decode_pages)
        .map(|(_, page_id)| page_id)
        .collect();
    let budget_bytes = args
        .budget_mb
        .checked_mul(1024 * 1024)
        .ok_or_else(|| "budget-mb is too large".to_string())?;
    let mut page_cache = DecodedPageCache::new(budget_bytes);
    let (decoded, stats) = page_cache.load_parallel(&cache, &selected, args.jobs)?;
    let decoded_records: u64 = decoded.iter().map(|page| page.records as u64).sum();
    let decoded_members: u64 = decoded.iter().map(|page| page.members).sum();
    let decoded_cells: usize = decoded.iter().map(|page| page.doc.cells.len()).sum();
    let scene = FrameScene::new(&cache, planned.plan, decoded)?;
    println!(
        "decode\tpages={}\tcells={}\trecords={}\tmembers={}\tresident_bytes={}\tcache_hit={}\tcache_miss={}\tread_us={}\tdecode_us={}\tworkers={}",
        scene.available_pages(),
        decoded_cells,
        decoded_records,
        decoded_members,
        stats.decoded_cache_bytes,
        stats.decoded_cache_hit,
        stats.decoded_cache_miss,
        stats.page_read_us,
        stats.page_decode_us,
        stats.decode_workers_used,
    );
    println!(
        "scene\tpartial={}\tavailable_pages={}\tdeferred_pages={}",
        scene.is_partial() as u8,
        scene.available_pages(),
        scene.deferred_pages().len(),
    );
    if let Some(path) = &args.out {
        let raster_request = GeometryRasterRequest {
            view: make_raster_view(&args, info.unit)?,
            width: args.width,
            height: args.height,
            background: [0, 0, 0, 255],
            foreground: [255, 255, 255, 255],
            workers: args.jobs,
            tile_size: args.tile_size,
        };
        let (mode, raster) = if args.styles.is_empty() {
            (
                "geometry-occupancy",
                render_geometry_occupancy(&scene, &raster_request)?,
            )
        } else {
            let cache_layers = cache.layers();
            let styles = args
                .styles
                .iter()
                .map(|style| parse_style(style, &cache_layers))
                .collect::<Result<Vec<_>, _>>()?;
            (
                "geometry-styled",
                render_geometry_styled(
                    &scene,
                    &StyledGeometryRasterRequest {
                        raster: raster_request,
                        layers: styles,
                        hierarchy_frames: args.frames,
                        mono: args.mono,
                    },
                )?,
            )
        };
        raster.frame.write_png(path)?;
        println!(
            "raster\tmode={}\tpartial={}\tworkers={}\ttiles={}\trect_record_tests={}\trect_member_paints={}\tpolygon_record_tests={}\tpolygon_member_paints={}\tpath_record_tests={}\tpath_member_paints={}\tframe_record_tests={}\tframe_member_paints={}\tdeferred_frame_tests={}\traster_us={}\tout={}",
            mode,
            raster.partial as u8,
            raster.stats.workers_used,
            raster.stats.tiles,
            raster.rect_record_tests,
            raster.rectangle_member_paints,
            raster.polygon_record_tests,
            raster.polygon_member_paints,
            raster.path_record_tests,
            raster.path_member_paints,
            raster.frame_record_tests,
            raster.frame_member_paints,
            raster.deferred_frame_tests,
            raster.stats.raster_us,
            path,
        );
    }
    Ok(())
}

fn make_request(args: &Args, unit: f64) -> Result<PlanRequest, String> {
    if !unit.is_finite() || unit <= 0.0 {
        return Err(format!("invalid cache unit: {unit}"));
    }
    let [x0, y0, x1, y1] = args.view_um;
    let to_dbu = |value: f64| -> Result<i64, String> {
        let scaled = value * unit;
        if !scaled.is_finite() || scaled < i64::MIN as f64 || scaled > i64::MAX as f64 {
            return Err(format!("view coordinate is outside i64 DBU range: {value}"));
        }
        Ok(scaled.round() as i64)
    };
    let view = ViewBox::new(to_dbu(x0)?, to_dbu(y0)?, to_dbu(x1)?, to_dbu(y1)?)?;
    let span_x = x1 - x0;
    let span_y = y1 - y0;
    if span_x <= 0.0 || span_y <= 0.0 {
        return Err("view must have positive width and height".to_string());
    }
    let px_per_um = (args.width as f64 / span_x).min(args.height as f64 / span_y);
    let px_per_dbu = px_per_um / unit;
    let cut_dbu = if args.cut_px == 0.0 {
        0
    } else {
        (args.cut_px / px_per_dbu).ceil() as i64
    };
    Ok(PlanRequest {
        view,
        cut_dbu,
        visible_layers: args.layers.clone(),
        depth: args.depth,
        px_per_dbu,
        exact: args.cut_px == 0.0,
    })
}

fn make_raster_view(args: &Args, unit: f64) -> Result<RasterViewBox, String> {
    if !unit.is_finite() || unit <= 0.0 {
        return Err(format!("invalid cache unit: {unit}"));
    }
    let [x0, y0, x1, y1] = args.view_um;
    RasterViewBox::new(x0 * unit, y0 * unit, x1 * unit, y1 * unit)
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    if raw.is_empty() || raw.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Err("missing arguments".to_string());
    }
    let cache = raw[0].clone();
    let mut view_um = None;
    let mut width = None;
    let mut height = None;
    let mut depth = u32::MAX;
    let mut cut_px = 0.0f64;
    let mut layers = None;
    let mut decode_pages = 1usize;
    let mut budget_mb = 1024u64;
    let mut jobs = 1u16;
    let mut tile_size = DEFAULT_TILE_SIZE;
    let mut styles = Vec::new();
    let mut frames = true;
    let mut mono = false;
    let mut out = None;
    let mut i = 1;
    while i < raw.len() {
        let flag = &raw[i];
        let value = raw
            .get(i + 1)
            .ok_or_else(|| format!("missing value for {flag}"))?;
        match flag.as_str() {
            "--view" => view_um = Some(parse_view(value)?),
            "--width" => width = Some(parse_positive(value, "width")?),
            "--height" => height = Some(parse_positive(value, "height")?),
            "--depth" => {
                depth = if value == "full" {
                    u32::MAX
                } else {
                    value
                        .parse()
                        .map_err(|_| format!("invalid depth: {value}"))?
                }
            }
            "--cut-px" => {
                cut_px = value
                    .parse()
                    .map_err(|_| format!("invalid cut-px: {value}"))?;
                if !cut_px.is_finite() || cut_px < 0.0 {
                    return Err(format!("invalid cut-px: {value}"));
                }
            }
            "--layers" => {
                layers = match value.as_str() {
                    "all" => None,
                    "none" => Some(Vec::new()),
                    _ => Some(value.split(',').map(str::to_string).collect()),
                };
            }
            "--decode-pages" => {
                decode_pages = value
                    .parse()
                    .map_err(|_| format!("invalid decode-pages: {value}"))?;
            }
            "--budget-mb" => {
                budget_mb = value
                    .parse()
                    .map_err(|_| format!("invalid budget-mb: {value}"))?;
            }
            "--jobs" => {
                jobs = parse_positive(value, "jobs")?
                    .try_into()
                    .map_err(|_| format!("jobs is too large: {value}"))?;
                if jobs > MAX_JOBS {
                    return Err(format!("jobs must be in 1..={MAX_JOBS}: {value}"));
                }
            }
            "--tile-px" => {
                let parsed = parse_positive(value, "tile-px")?;
                if parsed > u32::from(MAX_TILE_SIZE) {
                    return Err(format!("tile-px must be in 1..={MAX_TILE_SIZE}: {value}"));
                }
                tile_size = parsed
                    .try_into()
                    .map_err(|_| format!("tile-px is too large: {value}"))?;
            }
            "--style" => styles.push(value.clone()),
            "--frames" => frames = parse_on_off(value, "frames")?,
            "--mono" => mono = parse_on_off(value, "mono")?,
            "--out" => out = Some(value.clone()),
            _ => return Err(format!("unknown option: {flag}")),
        }
        i += 2;
    }
    Ok(Args {
        cache,
        view_um: view_um.ok_or_else(|| "--view is required".to_string())?,
        width: width.ok_or_else(|| "--width is required".to_string())?,
        height: height.ok_or_else(|| "--height is required".to_string())?,
        depth,
        cut_px,
        layers,
        decode_pages,
        budget_mb,
        jobs,
        tile_size,
        styles,
        frames,
        mono,
        out,
    })
}

fn parse_style(value: &str, layers: &[CacheLayer]) -> Result<LayerStyle, String> {
    let fields: Vec<&str> = value.split(',').collect();
    if !(3..=4).contains(&fields.len()) {
        return Err(format!("style requires L/D,#RRGGBB,FILL[,WIDTH]: {value}"));
    }
    let layer = layers
        .iter()
        .find(|layer| {
            fields[0] == layer.name
                || fields[0] == format!("{}/{}", layer.layer, layer.datatype)
                || fields[0] == format!("idx:{}", layer.index)
        })
        .ok_or_else(|| format!("styled layer not found: {}", fields[0]))?;
    let color = parse_color(fields[1])?;
    let fill = parse_fill(fields[2])?;
    let outline_width = if let Some(width) = fields.get(3) {
        let width = parse_positive(width, "style width")?;
        if width > 8 {
            return Err(format!("style width must be in 1..=8: {width}"));
        }
        width as u8
    } else {
        1
    };
    Ok(LayerStyle {
        layer_idx: layer.index,
        color,
        fill,
        outline_width,
    })
}

fn parse_color(value: &str) -> Result<[u8; 4], String> {
    let hex = value
        .strip_prefix('#')
        .ok_or_else(|| format!("style color must start with #: {value}"))?;
    if hex.len() != 6 && hex.len() != 8 {
        return Err(format!("style color must have 6 or 8 hex digits: {value}"));
    }
    let byte = |offset: usize| {
        u8::from_str_radix(&hex[offset..offset + 2], 16)
            .map_err(|_| format!("invalid style color: {value}"))
    };
    Ok([
        byte(0)?,
        byte(2)?,
        byte(4)?,
        if hex.len() == 8 { byte(6)? } else { 255 },
    ])
}

fn parse_fill(value: &str) -> Result<LayerFill, String> {
    match value {
        "solid" => Ok(LayerFill::Solid),
        "speckle" => Ok(LayerFill::Speckle),
        "clear" => Ok(LayerFill::Clear),
        _ => {
            let hex = value
                .strip_prefix("pat:")
                .ok_or_else(|| format!("unknown style fill: {value}"))?;
            if hex.len() != 64 {
                return Err(format!("pat fill requires exactly 64 hex digits: {value}"));
            }
            let mut rows = [0u16; 16];
            for (index, row) in rows.iter_mut().enumerate() {
                *row = u16::from_str_radix(&hex[index * 4..index * 4 + 4], 16)
                    .map_err(|_| format!("invalid pat fill: {value}"))?;
            }
            Ok(LayerFill::Pattern(rows))
        }
    }
}

fn parse_on_off(value: &str, name: &str) -> Result<bool, String> {
    match value {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(format!("{name} must be on or off: {value}")),
    }
}

fn parse_view(value: &str) -> Result<[f64; 4], String> {
    let values: Vec<f64> = value
        .split(',')
        .map(|part| {
            part.parse::<f64>()
                .map_err(|_| format!("invalid view: {value}"))
        })
        .collect::<Result<_, _>>()?;
    let view: [f64; 4] = values
        .try_into()
        .map_err(|_| format!("view requires four coordinates: {value}"))?;
    if view.iter().any(|coordinate| !coordinate.is_finite()) {
        return Err(format!("view contains non-finite coordinate: {value}"));
    }
    Ok(view)
}

fn parse_positive(value: &str, name: &str) -> Result<u32, String> {
    let parsed: u32 = value
        .parse()
        .map_err(|_| format!("invalid {name}: {value}"))?;
    if parsed == 0 {
        return Err(format!("{name} must be positive"));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimum_request() {
        let raw = [
            "cache", "--view", "-1,2,3,4", "--width", "100", "--height", "50",
        ]
        .map(str::to_string);
        let args = parse_args(&raw).unwrap();
        assert_eq!(args.view_um, [-1.0, 2.0, 3.0, 4.0]);
        assert_eq!(args.depth, u32::MAX);
        assert_eq!(args.decode_pages, 1);
        assert_eq!(args.budget_mb, 1024);
        assert_eq!(args.jobs, 1);
        assert_eq!(args.tile_size, DEFAULT_TILE_SIZE);

        let mut with_tile = raw.to_vec();
        with_tile.extend(["--tile-px".to_string(), "64".to_string()]);
        assert_eq!(parse_args(&with_tile).unwrap().tile_size, 64);

        let mut with_all_layers = raw.to_vec();
        with_all_layers.extend(["--layers".to_string(), "all".to_string()]);
        assert_eq!(parse_args(&with_all_layers).unwrap().layers, None);

        let mut with_no_layers = raw.to_vec();
        with_no_layers.extend(["--layers".to_string(), "none".to_string()]);
        assert_eq!(
            parse_args(&with_no_layers).unwrap().layers,
            Some(Vec::new())
        );
    }

    #[test]
    fn rejects_unknown_option() {
        let raw = [
            "cache", "--view", "0,0,1,1", "--width", "1", "--height", "1", "--bogus", "x",
        ]
        .map(str::to_string);
        assert!(parse_args(&raw).unwrap_err().contains("unknown option"));
    }

    #[test]
    fn parses_styled_layer_and_pattern_contract() {
        let layers = [CacheLayer {
            index: 3,
            layer: 10,
            datatype: 2,
            name: "M1".to_string(),
        }];
        assert_eq!(
            parse_style("10/2,#12aBef80,speckle,4", &layers).unwrap(),
            LayerStyle {
                layer_idx: 3,
                color: [0x12, 0xab, 0xef, 0x80],
                fill: LayerFill::Speckle,
                outline_width: 4,
            }
        );
        let pattern = format!("M1,#ffffff,pat:{}", "8000".repeat(16));
        assert_eq!(
            parse_style(&pattern, &layers).unwrap().fill,
            LayerFill::Pattern([0x8000; 16])
        );
        assert!(parse_style("10/2,#ffffff,solid,9", &layers).is_err());
    }
}
