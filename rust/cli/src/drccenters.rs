//! Exact error bbox centers for the ungrouped marker renderer.
//!
//! Each original-order row is `(xmin + xmax, ymin + ymax)`, two little-endian
//! signed 64-bit integers in raw result DB coordinates. Keeping twice the
//! center preserves half-DBU positions without float rounding. Workers decode
//! independent 64-error pack blocks without allocating point or error objects.
//! The coordinator owns cache identity, publication, and removal of incomplete
//! staging output on error/cancellation. Existing files are never overwritten.
//! Protocol 2 adds optional `--ready PATH`: one byte per 65,536-row task,
//! initially zero. A byte becomes 1 only after that entire task's center data
//! has been written. Readers may mmap and consume ready tasks while we run;
//! task completion order is arbitrary. The raw center cache format is unchanged.

use crate::drcmeasure::{add_coord, size, varint, Pack};
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

const BLOCK: usize = 64;
const CHUNK_BLOCKS: usize = 1024;
const TASK_ROWS: usize = CHUNK_BLOCKS * BLOCK;
const ROW_BYTES: usize = 16;

fn decode_centers(data: &[u8], count: usize, output: &mut Vec<u8>) -> Result<(), String> {
    let (mut pos, mut pfx, mut pfy) = (0usize, 0i64, 0i64);
    for _ in 0..count {
        let encoded = varint(data, &mut pos)?;
        let npts = size(encoded >> 1)?;
        if npts == 0 || npts > data.len().saturating_sub(pos) / 2 {
            return Err("invalid/truncated coordinate point count".into());
        }
        let mut x = add_coord(pfx, varint(data, &mut pos)?)?;
        let mut y = add_coord(pfy, varint(data, &mut pos)?)?;
        pfx = x;
        pfy = y;
        let (mut xmin, mut xmax, mut ymin, mut ymax) = (x, x, y, y);
        for _ in 1..npts {
            x = add_coord(x, varint(data, &mut pos)?)?;
            y = add_coord(y, varint(data, &mut pos)?)?;
            xmin = xmin.min(x);
            xmax = xmax.max(x);
            ymin = ymin.min(y);
            ymax = ymax.max(y);
        }
        let cx = xmin
            .checked_add(xmax)
            .ok_or("error center x overflows signed 64-bit coordinates")?;
        let cy = ymin
            .checked_add(ymax)
            .ok_or("error center y overflows signed 64-bit coordinates")?;
        output.extend_from_slice(&cx.to_le_bytes());
        output.extend_from_slice(&cy.to_le_bytes());
    }
    if pos != data.len() {
        return Err("coordinate block contains trailing or uncounted data".into());
    }
    Ok(())
}

#[cfg(unix)]
fn write_at(file: &File, mut bytes: &[u8], mut offset: u64) -> Result<(), String> {
    use std::os::unix::fs::FileExt;
    while !bytes.is_empty() {
        match file.write_at(bytes, offset) {
            Ok(0) => return Err("zero-length center output write".into()),
            Ok(n) => {
                bytes = &bytes[n..];
                offset += n as u64;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("write error centers: {}", error)),
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn write_at(_file: &File, _bytes: &[u8], _offset: u64) -> Result<(), String> {
    Err("parallel center preprocessing requires positioned file writes on Unix".into())
}

fn create_output(path: &Path, errors: usize) -> Result<File, String> {
    let length = (errors as u64)
        .checked_mul(ROW_BYTES as u64)
        .ok_or("error center output size overflow")?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("create {}: {}", path.display(), e))?;
    file.set_len(length)
        .map_err(|e| format!("size error center output: {}", e))?;
    Ok(file)
}

fn task_count(pack: &Pack<'_>) -> usize {
    pack.block_count / CHUNK_BLOCKS + usize::from(pack.block_count % CHUNK_BLOCKS != 0)
}

fn create_ready(path: &Path, tasks: usize) -> Result<File, String> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("create {}: {}", path.display(), e))?;
    // Extending a newly created file guarantees zero bytes, including holes.
    file.set_len(tasks as u64)
        .map_err(|e| format!("size error center readiness bitmap: {}", e))?;
    Ok(file)
}

fn output_target(path: &Path) -> Result<PathBuf, String> {
    let name = path.file_name().ok_or("output path must name a file")?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = std::fs::canonicalize(parent)
        .map_err(|e| format!("resolve output parent {}: {}", parent.display(), e))?;
    Ok(parent.join(name))
}

fn validate_paths(input: &Path, output: &Path, ready: Option<&Path>) -> Result<(), String> {
    let input = std::fs::canonicalize(input).map_err(|e| format!("resolve DRC pack: {}", e))?;
    let output = output_target(output)?;
    if output == input {
        return Err("center output must differ from input pack".into());
    }
    if let Some(ready) = ready {
        let ready = output_target(ready)?;
        if ready == input || ready == output {
            return Err("readiness bitmap must differ from input pack and center output".into());
        }
    }
    Ok(())
}

fn run(pack: &Pack<'_>, output: &File, ready: Option<&File>, jobs: usize) -> Result<(), String> {
    let tasks = task_count(pack);
    let jobs = jobs.min(tasks.max(1));
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    std::thread::scope(|scope| -> Result<(), String> {
        let mut workers = Vec::with_capacity(jobs);
        for _ in 0..jobs {
            let (next, done, failed) = (&next, &done, &failed);
            workers.push(scope.spawn(move || -> Result<(), String> {
                let result = (|| {
                    // 1 MiB per worker, independent of rule size or geometry complexity.
                    let mut centers = Vec::with_capacity(TASK_ROWS * ROW_BYTES);
                    loop {
                        if failed.load(Ordering::Relaxed) {
                            break;
                        }
                        let task = next.fetch_add(1, Ordering::Relaxed);
                        if task >= tasks {
                            break;
                        }
                        let first = task * CHUNK_BLOCKS;
                        let last = (first + CHUNK_BLOCKS).min(pack.block_count);
                        centers.clear();
                        for block in first..last {
                            let (data, count) = pack.block(block)?;
                            decode_centers(data, count, &mut centers)?;
                        }
                        let row = (first * BLOCK) as u64;
                        write_at(output, &centers, row * ROW_BYTES as u64)?;
                        // Publish only after all bytes are visible to mmap/pread
                        // consumers. No fsync is needed for this live-process
                        // handoff; cache publication remains the caller's job.
                        if let Some(ready) = ready {
                            write_at(ready, &[1], task as u64)?;
                        }
                        done.fetch_add(centers.len() / ROW_BYTES, Ordering::Relaxed);
                    }
                    Ok(())
                })();
                if result.is_err() {
                    failed.store(true, Ordering::Relaxed);
                }
                result
            }));
        }
        for worker in workers {
            match worker.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => return Err(error),
                Err(_) => return Err("error center worker panicked".into()),
            }
        }
        Ok(())
    })?;
    if done.load(Ordering::Relaxed) != pack.errors {
        return Err("incomplete error center output".into());
    }
    Ok(())
}

fn command(args: &[String]) -> Result<(), String> {
    if args == ["--protocol"] {
        println!("2");
        return Ok(());
    }
    let path = args
        .first()
        .filter(|value| !value.starts_with("--"))
        .ok_or(
        "usage: floe-index drc-centers PACK --rule-index N --out PATH [--jobs N] [--ready PATH]",
    )?;
    let (mut rule, mut output, mut ready) = (None, None, None);
    let mut jobs = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(4);
    let mut i = 1;
    while i < args.len() {
        let option = &args[i];
        let value = args
            .get(i + 1)
            .ok_or_else(|| format!("{} requires a value", option))?;
        match option.as_str() {
            "--rule-index" => {
                rule = Some(value.parse::<usize>().map_err(|_| "invalid rule index")?)
            }
            "--out" => output = Some(Path::new(value)),
            "--ready" => ready = Some(Path::new(value)),
            "--jobs" => {
                jobs = value.parse().map_err(|_| "invalid center worker count")?;
                if jobs == 0 || jobs > 256 {
                    return Err("center worker count must be between 1 and 256".into());
                }
            }
            _ => return Err(format!("unknown drc-centers option {}", option)),
        }
        i += 2;
    }
    let rule = rule.ok_or("--rule-index is required")?;
    let output = output.ok_or("--out is required")?;
    let file = File::open(path).map_err(|e| format!("open DRC pack: {}", e))?;
    let map = unsafe { memmap2::Mmap::map(&file) }.map_err(|e| format!("map DRC pack: {}", e))?;
    let pack = Pack::parse(&map, rule)?;
    validate_paths(Path::new(path), output, ready)?;
    let file = create_output(output, pack.errors)?;
    let ready = ready
        .map(|path| create_ready(path, task_count(&pack)))
        .transpose()?;
    let began = Instant::now();
    run(&pack, &file, ready.as_ref(), jobs)?;
    eprintln!(
        "[drc-centers] {} errors in {:.3}s",
        pack.errors,
        began.elapsed().as_secs_f64()
    );
    Ok(())
}

pub fn drccenters_cmd(args: &[String]) {
    if let Err(error) = command(args) {
        eprintln!("[drc-centers] {}", error);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drcmeasure::tests::fixture;
    use std::path::PathBuf;

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("floe-centers-{}-{}", std::process::id(), stamp));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn put_varint(mut value: u64, output: &mut Vec<u8>) {
        while value >= 0x80 {
            output.push((value as u8 & 0x7f) | 0x80);
            value >>= 7;
        }
        output.push(value as u8);
    }
    fn encode(geometries: &[(u8, Vec<(i64, i64)>)]) -> Vec<u8> {
        let mut output = Vec::new();
        let (mut pfx, mut pfy) = (0i64, 0i64);
        for (kind, points) in geometries {
            put_varint(((points.len() as u64) << 1) | u64::from(*kind), &mut output);
            let (mut px, mut py) = (pfx, pfy);
            for &(x, y) in points {
                for delta in [x.checked_sub(px).unwrap(), y.checked_sub(py).unwrap()] {
                    put_varint(((delta as u64) << 1) ^ ((delta >> 63) as u64), &mut output);
                }
                (px, py) = (x, y);
            }
            if let Some(&(x, y)) = points.first() {
                (pfx, pfy) = (x, y);
            }
        }
        output
    }
    fn rows(bytes: &[u8]) -> Vec<(i64, i64)> {
        bytes
            .chunks_exact(16)
            .map(|row| {
                (
                    i64::from_le_bytes(row[..8].try_into().unwrap()),
                    i64::from_le_bytes(row[8..].try_into().unwrap()),
                )
            })
            .collect()
    }

    #[test]
    fn exact_centers_cover_polygon_edge_point_and_half_negative_dbu() {
        // The bbox, not the vertex mean, defines the center. Odd sums preserve
        // the fractional DBU for both negative and positive coordinates.
        let geometry = vec![
            (0, vec![(-8, -5), (5, -5), (5, 8), (-8, 8), (-8, -5)]),
            (1, vec![(9, -2), (14, -2), (10, 7), (10, 12)]),
            (0, vec![(-3, 4)]),
        ];
        let mut output = Vec::new();
        decode_centers(&encode(&geometry), geometry.len(), &mut output).unwrap();
        assert_eq!(rows(&output), vec![(-3, 3), (23, 10), (-6, 8)]);
    }

    #[test]
    fn malformed_geometry_and_coordinate_center_overflow_fail() {
        for (data, count) in [
            (vec![0], 1),
            (vec![2, 0], 1),
            (vec![0xff; 10], 1),
            (vec![2, 0, 0, 0], 1),
        ] {
            assert!(decode_centers(&data, count, &mut Vec::new()).is_err());
        }
        let data = encode(&[(0, vec![(i64::MAX, 0)])]);
        assert!(decode_centers(&data, 1, &mut Vec::new())
            .unwrap_err()
            .contains("center x overflows"));
        let data = encode(&[(0, vec![(0, i64::MIN)])]);
        assert!(decode_centers(&data, 1, &mut Vec::new())
            .unwrap_err()
            .contains("center y overflows"));
        // First point is MAX, then a positive delta produces invalid geometry.
        let mut data = Vec::new();
        put_varint(4, &mut data);
        put_varint((i64::MAX as u64) << 1, &mut data);
        data.extend_from_slice(&[0, 2, 0]);
        assert!(decode_centers(&data, 1, &mut Vec::new())
            .unwrap_err()
            .contains("coordinate delta overflow"));
    }

    #[test]
    fn parallel_outputs_preserve_order_and_empty_rules_work() {
        let temp = Temp::new();
        let n = CHUNK_BLOCKS * BLOCK * 3 + 3;
        let mut data = fixture(n);
        // Vary the first rectangle's width in every block so misplaced parallel
        // writes or lost original-order alignment cannot pass a constant test.
        let starts: Vec<usize> = {
            let pack = Pack::parse(&data, 0).unwrap();
            (0..pack.block_count)
                .map(|block| {
                    pack.block(block).unwrap().0.as_ptr() as usize - data.as_ptr() as usize
                })
                .collect()
        };
        for (block, start) in starts.into_iter().enumerate() {
            let width = (block % 20 + 1) as u8;
            data[start + 3] = width * 2;
            data[start + 8] = width * 2 - 1;
        }
        let pack = Pack::parse(&data, 0).unwrap();
        for jobs in [1, 4] {
            let path = temp.0.join(jobs.to_string());
            let output = create_output(&path, n).unwrap();
            let ready_path = temp.0.join(format!("ready-{}", jobs));
            let ready = if jobs == 4 {
                Some(create_ready(&ready_path, task_count(&pack)).unwrap())
            } else {
                None
            };
            if ready.is_some() {
                assert_eq!(
                    std::fs::read(&ready_path).unwrap(),
                    vec![0; task_count(&pack)]
                );
            }
            run(&pack, &output, ready.as_ref(), jobs).unwrap();
            if ready.is_some() {
                assert_eq!(
                    std::fs::read(ready_path).unwrap(),
                    vec![1; task_count(&pack)]
                );
            }
            assert!(create_output(&path, n).is_err());
        }
        let one = std::fs::read(temp.0.join("1")).unwrap();
        assert_eq!(one, std::fs::read(temp.0.join("4")).unwrap());
        assert_eq!(one.len(), n * ROW_BYTES);
        for (i, row) in rows(&one).into_iter().enumerate() {
            let width = if i % BLOCK == 0 {
                (i / BLOCK % 20 + 1) as i64
            } else {
                20
            };
            assert_eq!(row, (width, 100));
        }
        let data = fixture(0);
        let pack = Pack::parse(&data, 0).unwrap();
        let path = temp.0.join("empty");
        let output = create_output(&path, 0).unwrap();
        let ready_path = temp.0.join("empty-ready");
        let ready = create_ready(&ready_path, task_count(&pack)).unwrap();
        run(&pack, &output, Some(&ready), 4).unwrap();
        assert_eq!(std::fs::metadata(path).unwrap().len(), 0);
        assert_eq!(std::fs::metadata(ready_path).unwrap().len(), 0);
    }

    #[test]
    fn readiness_never_publishes_failed_decode_or_output_write() {
        let temp = Temp::new();
        let n = TASK_ROWS + BLOCK;
        let mut data = fixture(n);
        let invalid = {
            let pack = Pack::parse(&data, 0).unwrap();
            pack.block(CHUNK_BLOCKS).unwrap().0.as_ptr() as usize - data.as_ptr() as usize
        };
        data[invalid] = 0; // Corrupt only task 1; task 0 must remain consumable.
        let pack = Pack::parse(&data, 0).unwrap();
        let output_path = temp.0.join("partial");
        let ready_path = temp.0.join("partial-ready");
        let output = create_output(&output_path, n).unwrap();
        let ready = create_ready(&ready_path, task_count(&pack)).unwrap();
        assert!(run(&pack, &output, Some(&ready), 1).is_err());
        assert_eq!(std::fs::read(&ready_path).unwrap(), vec![1, 0]);
        let raw = std::fs::read(output_path).unwrap();
        assert!(rows(&raw[..TASK_ROWS * ROW_BYTES])
            .iter()
            .all(|&row| row == (20, 100)));
        assert!(raw[TASK_ROWS * ROW_BYTES..].iter().all(|&v| v == 0));

        let data = fixture(65);
        let pack = Pack::parse(&data, 0).unwrap();
        let path = temp.0.join("readonly");
        drop(create_output(&path, 65).unwrap());
        let readonly = File::open(path).unwrap();
        let path = temp.0.join("write-failed-ready");
        let ready = create_ready(&path, 1).unwrap();
        assert!(run(&pack, &readonly, Some(&ready), 1).is_err());
        assert_eq!(std::fs::read(path).unwrap(), vec![0]);
    }

    #[test]
    fn ready_paths_are_distinct_and_existing_files_are_preserved() {
        let temp = Temp::new();
        let input = temp.0.join("input.tray");
        let output = temp.0.join("centers.bin");
        let ready = temp.0.join("ready.bin");
        let original = fixture(65);
        std::fs::write(&input, &original).unwrap();
        assert!(validate_paths(&input, &input, None).is_err());
        assert!(validate_paths(&input, &output, Some(&input)).is_err());
        assert!(validate_paths(&input, &output, Some(&temp.0.join("./centers.bin"))).is_err());
        let args = |ready: &Path| {
            vec![
                input.to_string_lossy().into_owned(),
                "--rule-index".into(),
                "0".into(),
                "--out".into(),
                output.to_string_lossy().into_owned(),
                "--ready".into(),
                ready.to_string_lossy().into_owned(),
            ]
        };
        assert!(command(&args(&output)).is_err());
        assert!(!output.exists());
        std::fs::write(&ready, b"keep readiness file").unwrap();
        assert!(command(&args(&ready)).is_err());
        assert_eq!(std::fs::read(&ready).unwrap(), b"keep readiness file");
        assert_eq!(std::fs::read(&input).unwrap(), original);
        assert!(create_ready(&ready, 100).is_err());
    }

    #[test]
    fn invalid_pack_or_options_do_not_create_output_and_existing_files_survive() {
        let temp = Temp::new();
        let input = temp.0.join("bad.tray");
        let output = temp.0.join("centers.bin");
        std::fs::write(&input, b"invalid DRC pack").unwrap();
        let mut args = vec![
            input.to_string_lossy().into_owned(),
            "--rule-index".into(),
            "0".into(),
            "--out".into(),
            output.to_string_lossy().into_owned(),
        ];
        assert!(command(&args).is_err());
        assert!(!output.exists());
        std::fs::write(&input, fixture(65)).unwrap();
        args[2] = "1".into();
        assert!(command(&args).is_err());
        assert!(!output.exists());
        args[2] = "0".into();
        args.extend(["--jobs".into(), "0".into()]);
        assert!(command(&args).is_err());
        assert!(!output.exists());
        args.truncate(5);
        std::fs::write(&output, b"preserve me").unwrap();
        assert!(command(&args).is_err());
        assert_eq!(std::fs::read(&output).unwrap(), b"preserve me");
        assert!(create_output(&temp.0.join("missing").join("centers"), 1).is_err());
        assert!(command(&[]).is_err());
    }
}
