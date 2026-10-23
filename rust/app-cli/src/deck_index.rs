use crate::{ready::Waiter, Out};
use floe_app_core::{
    check_cancelled,
    index::{Action, IndexOptions},
    jobdeck::index::{DeckIndexEntry, DeckIndexPlan},
    ErrorKind, Result,
};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

pub fn run(
    source: &Path,
    levels: Option<BTreeSet<i64>>,
    options: &IndexOptions,
    waiter: &mut Waiter,
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
    if plan.full_over_light > 0 {
        println!(
            "[jobdeck] index     : {} light cache(s) rebuilt as the full index (--full)",
            plan.full_over_light
        );
    }
    if plan.aliases > 0 {
        println!(
            "[jobdeck] aliases   : {} source name(s) share a cache destination",
            plan.aliases
        );
    }
    run_entries(&plan.todo, plan.kept, Out::Stdout, waiter, cancelled)
}

/// Index `todo` one source after the other, saying each on `out`, and the
/// counts (`kept` already indexed): 0, 2 with failures, 75 with sources
/// another run held (BUSY) - or, `waiter` on (--wait), waited for: a source
/// whose run ended is then kept (its cache is that run's) or built.
pub(crate) fn run_entries(
    todo: &[DeckIndexEntry],
    kept: usize,
    out: Out,
    waiter: &mut Waiter,
    cancelled: &AtomicUsize,
) -> Result<i32> {
    let (mut built, mut failed, mut busy, mut kept) = (0, 0, 0, kept);
    let batch_started = Instant::now();
    for (n, entry) in todo.iter().enumerate() {
        check_cancelled(cancelled)?;
        let label = if entry.options.occupancy_only {
            "occupancy"
        } else {
            "index    "
        };
        out.line(&format!(
            "[jobdeck] {label} : ({}/{}) {}",
            n + 1,
            todo.len(),
            entry.tc
        ));
        out.flush()?;
        let started = Instant::now();
        let result = loop {
            let result = match super::execute_index(&entry.source, &entry.options, out, cancelled) {
                Err(error) if error.kind == ErrorKind::Cancelled => return Err(error),
                result => result,
            };
            let refused = matches!(result, Ok((floe_vfs::lock::BUSY_EXIT, _)))
                || matches!(&result, Err(error) if error.kind == ErrorKind::Busy);
            if !(refused && waiter.on()) {
                break result;
            }
            // readers in its way only where it has a cache to replace
            let readers = entry.options.force || floe_app_core::cache::cache_path(&entry.source).is_ok_and(|d| d.exists());
            waiter.wait_to_write(&crate::ready::layout_key(&entry.source)?, readers)?;
        };
        let signal = cancelled.load(Ordering::Relaxed);
        if signal != 0 {
            return Ok(128 + signal as i32);
        }
        let elapsed = batch_started.elapsed().as_secs_f64();
        let remaining = elapsed / (n + 1) as f64 * (todo.len() - n - 1) as f64;
        let progress = format!("({}/{})", n + 1, todo.len());
        let timing = format!("{} elapsed, ~{} left", hms(elapsed), hms(remaining));
        match result {
            Ok((code, _)) if matches!(code, 130 | 143) => return Ok(code),
            Ok((0, Action::Reuse | Action::OccupancyPresent)) => kept += 1,
            Ok((0, _)) => {
                built += 1;
                out.line(&format!(
                    "[jobdeck] {label} : {progress} ok {} ({:.1}s; {timing})",
                    entry.tc,
                    started.elapsed().as_secs_f64()
                ));
            }
            // another run indexes the source, or (a rebuild) someone has it
            // open (floe_vfs::lock): not a failure of this deck - said with
            // who, and the next source goes on
            Ok((floe_vfs::lock::BUSY_EXIT, _)) => {
                busy += 1;
                out.line(&format!(
                    "[jobdeck] {label} : {progress} BUSY {} - another run or a viewer holds it (see the [lock] line above; {timing})",
                    entry.tc
                ));
            }
            Err(error) if error.kind == ErrorKind::Busy => {
                busy += 1;
                eprintln!("[lock] {}", error.message);
                out.line(&format!(
                    "[jobdeck] {label} : {progress} BUSY {} - another run or a viewer holds it (see the [lock] line above; {timing})",
                    entry.tc
                ));
            }
            Ok((code, _)) => {
                failed += 1;
                // the code in words where there are some (STREAMING_INDEX_PLAN
                // §5: 137 is the kernel's kill, 71 the memory guard's stop)
                out.line(&format!(
                    "[jobdeck] {label} : {progress} FAILED {} (exit {code}{}; {timing})",
                    entry.tc,
                    floe_app_core::index::exit_words(code)
                        .map(|w| format!(" - {w}"))
                        .unwrap_or_default()
                ));
            }
            Err(error) => {
                failed += 1;
                out.line(&format!(
                    "[jobdeck] {label} : {progress} FAILED {} ({error}; {timing})",
                    entry.tc
                ));
            }
        }
        out.flush()?;
    }
    // the busy count only when there are some (scripts read this line)
    let busy_text = if busy > 0 {
        format!("{busy} busy, ")
    } else {
        String::new()
    };
    out.line(&format!("[jobdeck] index     : {built} built, {failed} failed, {busy_text}{kept} kept"));
    Ok(if failed > 0 {
        2
    } else if busy > 0 {
        floe_vfs::lock::BUSY_EXIT
    } else {
        0
    })
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
