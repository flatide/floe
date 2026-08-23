use floe_render_core::Cache;
use std::env;
use std::process::ExitCode;

const DEFAULT_JOBS: u16 = 8;
const DEFAULT_CHUNK_PAGES: u32 = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PathShape {
    degenerate: bool,
    u_turn: bool,
    non_manhattan: bool,
    vertices: usize,
}

#[derive(Debug, Default)]
struct Inventory {
    pages: u64,
    encoded_bytes: u64,
    path_records: u64,
    path_members: u64,
    vertices: u64,
    max_vertices: usize,
    degenerate: u64,
    u_turn: u64,
    non_manhattan: u64,
    zero_half_width: u64,
    extension_zero: u64,
    extension_half_width: u64,
    extension_other: u64,
    extension_negative: u64,
}

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!(
                "error: {error}\nusage: path-inventory [--jobs N] [--chunk-pages N] CACHE..."
            );
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let (jobs, chunk_pages, caches) = parse_args(&args)?;
    for cache_path in caches {
        let cache = Cache::open(&cache_path)?;
        let page_count = cache.info().pages;
        let mut inventory = Inventory::default();
        let mut first = 0u32;
        while first < page_count {
            let end = first.saturating_add(chunk_pages).min(page_count);
            let page_ids: Vec<u32> = (first..end).collect();
            let (pages, _) = cache.decode_pages_parallel(&page_ids, jobs)?;
            for page in pages {
                inventory.pages = inventory.pages.saturating_add(1);
                inventory.encoded_bytes = inventory
                    .encoded_bytes
                    .saturating_add(u64::from(page.encoded_bytes));
                for cell in &page.doc.cells {
                    for path in &cell.paths {
                        inventory.path_records = inventory.path_records.saturating_add(1);
                        inventory.path_members =
                            inventory.path_members.saturating_add(path.rep.members());
                        let shape = classify_path(&path.pts);
                        inventory.vertices = inventory
                            .vertices
                            .saturating_add(shape.vertices.try_into().unwrap_or(u64::MAX));
                        inventory.max_vertices = inventory.max_vertices.max(shape.vertices);
                        inventory.degenerate = inventory
                            .degenerate
                            .saturating_add(u64::from(shape.degenerate));
                        inventory.u_turn = inventory.u_turn.saturating_add(u64::from(shape.u_turn));
                        inventory.non_manhattan = inventory
                            .non_manhattan
                            .saturating_add(u64::from(shape.non_manhattan));
                        inventory.zero_half_width = inventory
                            .zero_half_width
                            .saturating_add(u64::from(path.hw == 0));
                        for extension in [path.es, path.ee] {
                            if extension < 0 {
                                inventory.extension_negative =
                                    inventory.extension_negative.saturating_add(1);
                            }
                            if extension == 0 {
                                inventory.extension_zero =
                                    inventory.extension_zero.saturating_add(1);
                            } else if extension == path.hw {
                                inventory.extension_half_width =
                                    inventory.extension_half_width.saturating_add(1);
                            } else {
                                inventory.extension_other =
                                    inventory.extension_other.saturating_add(1);
                            }
                        }
                    }
                }
            }
            first = end;
        }
        println!(
            "path-inventory\tcache={}\tpages={}\tencoded_bytes={}\tpath_records={}\tpath_members={}\tvertices={}\tmax_vertices={}\tdegenerate={}\tu_turn={}\tnon_manhattan={}\tzero_half_width={}\text_zero={}\text_half_width={}\text_other={}\text_negative={}",
            cache_path,
            inventory.pages,
            inventory.encoded_bytes,
            inventory.path_records,
            inventory.path_members,
            inventory.vertices,
            inventory.max_vertices,
            inventory.degenerate,
            inventory.u_turn,
            inventory.non_manhattan,
            inventory.zero_half_width,
            inventory.extension_zero,
            inventory.extension_half_width,
            inventory.extension_other,
            inventory.extension_negative,
        );
    }
    Ok(())
}

fn parse_args(args: &[String]) -> Result<(u16, u32, Vec<String>), String> {
    let mut jobs = DEFAULT_JOBS;
    let mut chunk_pages = DEFAULT_CHUNK_PAGES;
    let mut caches = Vec::new();
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--jobs" => {
                index += 1;
                jobs = parse_positive(args.get(index), "jobs")?
                    .try_into()
                    .map_err(|_| "jobs must be in 1..=256".to_string())?;
                if jobs > 256 {
                    return Err("jobs must be in 1..=256".to_string());
                }
            }
            "--chunk-pages" => {
                index += 1;
                chunk_pages = parse_positive(args.get(index), "chunk-pages")?
                    .try_into()
                    .map_err(|_| "chunk-pages is too large".to_string())?;
            }
            option if option.starts_with('-') => {
                return Err(format!("unknown option: {option}"));
            }
            cache => caches.push(cache.to_string()),
        }
        index += 1;
    }
    if caches.is_empty() {
        return Err("at least one cache is required".to_string());
    }
    Ok((jobs, chunk_pages, caches))
}

fn parse_positive(value: Option<&String>, field: &str) -> Result<u64, String> {
    let value = value.ok_or_else(|| format!("missing value for --{field}"))?;
    let parsed = value
        .parse::<u64>()
        .map_err(|_| format!("invalid --{field}: {value}"))?;
    if parsed == 0 {
        return Err(format!("--{field} must be positive"));
    }
    Ok(parsed)
}

fn classify_path(points: &[(i64, i64)]) -> PathShape {
    let mut spine = Vec::with_capacity(points.len());
    for &point in points {
        if spine.last() == Some(&point) {
            continue;
        }
        if spine.len() >= 2 {
            let a = spine[spine.len() - 2];
            let b = spine[spine.len() - 1];
            let first = direction(a, b);
            let second = direction(b, point);
            if first == second {
                spine.pop();
            }
        }
        spine.push(point);
    }
    if spine.len() < 2 {
        return PathShape {
            degenerate: true,
            vertices: spine.len(),
            ..PathShape::default()
        };
    }

    let directions: Vec<(i128, i128)> = spine
        .windows(2)
        .map(|segment| direction(segment[0], segment[1]))
        .collect();
    let non_manhattan = directions.iter().any(|&(dx, dy)| dx != 0 && dy != 0);
    let u_turn = directions
        .windows(2)
        .any(|pair| pair[0] == (-pair[1].0, -pair[1].1));
    PathShape {
        degenerate: false,
        u_turn,
        non_manhattan,
        vertices: spine.len(),
    }
}

fn direction(start: (i64, i64), end: (i64, i64)) -> (i128, i128) {
    let dx = i128::from(end.0) - i128::from(start.0);
    let dy = i128::from(end.1) - i128::from(start.1);
    let divisor = gcd(dx.unsigned_abs(), dy.unsigned_abs()).max(1);
    (dx / divisor as i128, dy / divisor as i128)
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_normalized_path_shapes() {
        assert_eq!(
            classify_path(&[(0, 0), (5, 0), (10, 0)]),
            PathShape {
                vertices: 2,
                ..PathShape::default()
            }
        );
        assert!(classify_path(&[(0, 0), (5, 0), (0, 0)]).u_turn);
        assert!(classify_path(&[(0, 0), (5, 3)]).non_manhattan);
        assert!(classify_path(&[(2, 2), (2, 2)]).degenerate);
    }

    #[test]
    fn rejects_invalid_inventory_arguments() {
        assert!(parse_args(&[]).is_err());
        assert!(parse_args(&["--jobs".into(), "0".into(), "x".into()]).is_err());
        assert!(parse_args(&["--chunk-pages".into(), "0".into(), "x".into()]).is_err());
    }
}
