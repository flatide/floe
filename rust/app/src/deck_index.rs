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
        match result {
            Ok((code, _)) if matches!(code, 130 | 143) => return Ok(code),
            Ok((0, Action::Reuse | Action::OccupancyPresent)) => kept += 1,
            Ok((0, _)) => {
                built += 1;
                println!(
                    "[jobdeck] {label} : ok {} ({:.1}s)",
                    entry.tc,
                    started.elapsed().as_secs_f64()
                );
            }
            Ok((code, _)) => {
                failed += 1;
                eprintln!("[jobdeck] {label} : FAILED {} (exit {code})", entry.tc);
            }
            Err(error) => {
                failed += 1;
                eprintln!("[jobdeck] {label} : FAILED {} ({error})", entry.tc);
            }
        }
    }
    println!("[jobdeck] index     : {built} built, {failed} failed, {kept} kept");
    Ok(if failed == 0 { 0 } else { 2 })
}
