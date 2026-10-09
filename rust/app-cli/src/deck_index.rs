use floe_app_core::{
    check_cancelled,
    index::{Action, IndexOptions},
    jobdeck::index::DeckIndexPlan,
    ErrorKind, Result,
};
use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

pub fn run(
    source: &Path,
    levels: Option<BTreeSet<i64>>,
    options: &IndexOptions,
    cancelled: &AtomicUsize,
) -> Result<i32> {
    let plan = DeckIndexPlan::prepare(source, levels, options, cancelled)?;
    let list = |ids: &BTreeSet<i64>| ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
    if let Some(ids) = &plan.selected {
        println!(
            "[jobdeck] levels    : {} of {}",
            list(ids),
            list(&plan.levels)
        );
    }
    for info in plan.catalog.infos.values().filter(|i| !i.ok()) {
        println!(
            "[jobdeck] source    : {} {} ({})",
            info.tc,
            info.status.to_uppercase(),
            info.error
        );
    }
    if plan.kept > 0 {
        println!(
            "[jobdeck] index     : {} source(s) already indexed",
            plan.kept
        );
    }
    if plan.aliases > 0 {
        println!(
            "[jobdeck] aliases   : {} source name(s) share a cache destination",
            plan.aliases
        );
    }
    let (mut built, mut failed, mut kept) = (0, 0, plan.kept);
    let batch_started = Instant::now();
    for (n, entry) in plan.todo.iter().enumerate() {
        check_cancelled(cancelled)?;
        let label = if entry.options.occupancy_only {
            "occupancy"
        } else {
            "index    "
        };
        println!(
            "[jobdeck] {label} : ({}/{}) {}",
            n + 1,
            plan.todo.len(),
            entry.tc
        );
        std::io::stdout().flush()?;
        let started = Instant::now();
        let result = match super::execute_index(&entry.source, &entry.options, cancelled) {
            Err(error) if error.kind == ErrorKind::Cancelled => return Err(error),
            result => result,
        };
        let signal = cancelled.load(Ordering::Relaxed);
        if signal != 0 {
            return Ok(128 + signal as i32);
        }
        let elapsed = batch_started.elapsed().as_secs_f64();
        let remaining = elapsed / (n + 1) as f64 * (plan.todo.len() - n - 1) as f64;
        let progress = format!("({}/{})", n + 1, plan.todo.len());
        let timing = format!("{} elapsed, ~{} left", hms(elapsed), hms(remaining));
        match result {
            Ok((code, _)) if matches!(code, 130 | 143) => return Ok(code),
            Ok((0, Action::Reuse | Action::OccupancyPresent)) => kept += 1,
            Ok((0, _)) => {
                built += 1;
                println!(
                    "[jobdeck] {label} : {progress} ok {} ({:.1}s; {timing})",
                    entry.tc,
                    started.elapsed().as_secs_f64()
                );
            }
            Ok((code, _)) => {
                failed += 1;
                eprintln!(
                    "[jobdeck] {label} : {progress} FAILED {} (exit {code}; {timing})",
                    entry.tc
                );
            }
            Err(error) => {
                failed += 1;
                eprintln!(
                    "[jobdeck] {label} : {progress} FAILED {} ({error}; {timing})",
                    entry.tc
                );
            }
        }
    }
    println!("[jobdeck] index     : {built} built, {failed} failed, {kept} kept");
    Ok(if failed == 0 { 0 } else { 2 })
}

fn hms(seconds: f64) -> String {
    let seconds = seconds.max(0.).round_ties_even() as u64;
    let (h, m, s) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn progress_times_match_python_rounding_and_hours() {
        for (seconds, expected) in [
            (0., "0:00"),
            (7., "0:07"),
            (221., "3:41"),
            (3735., "1:02:15"),
            (2.5, "0:02"),
            (3.5, "0:04"),
        ] {
            assert_eq!(super::hms(seconds), expected);
        }
    }
}
