//! Memory guards (docs/STREAMING_INDEX_PLAN.ko.md §5, S6).
//!
//! The field kill of 2026-10-19: the in-memory parse of a ~50 GB deck
//! source reached 361 GiB of the host's 376 at 160 s, then the host
//! swapped for 21 minutes before the OOM killer ended it with 137. Two
//! guards make that a one-line stop instead. The ceiling: the process's
//! anonymous resident size against 90 % of MemTotal (`FLOE_RSS_CEILING`),
//! checked once when a build starts, once before the cache is cleared and
//! by every heartbeat (the parse, the pipeline, the inventory, the
//! readers, the occupancy pass). The size pre-check of the in-memory
//! build: a file larger than MemAvailable is refused before it is read
//! (the parsed form is several times the file). Both end with
//! EXIT_MEMORY, say what the cache is left as and what to do.

/// exit status of a build stopped at the memory ceiling or refused for
/// its size - apart from the lock's 75, usage's 2, a failure's 1 and the
/// kernel's 137 (floe2's deck index words each of them)
pub const EXIT_MEMORY: i32 = 71;

/// what the cache is left as, for the stop line
pub const CACHE_UNCHANGED: &str = "the cache is unchanged";
pub const CACHE_INCOMPLETE: &str = "the cache has no commit marker and is rebuilt next time";
/// what to do, for the stop line
pub const ADVICE_STREAM: &str = "index with --stream (two passes; resident memory bounded by --mem)";
pub const ADVICE_BUDGET: &str = "lower --mem (the resident budget) or free memory on the host";

/// anonymous resident bytes of this process: Linux RssAnon (VmRSS on a
/// kernel without it), macOS phys_footprint; None elsewhere, where the
/// ceiling is inert. File-backed pages - the streaming build's mapping of
/// the source - do not count: they are the page cache's to drop.
pub fn rss_anon_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string("/proc/self/status").ok()?;
        let kb = |key: &str| -> Option<u64> {
            s.lines()
                .find_map(|l| l.strip_prefix(key))
                .and_then(|rest| rest.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
        };
        return kb("RssAnon:").or_else(|| kb("VmRSS:")).map(|k| k * 1024);
    }
    #[cfg(target_os = "macos")]
    {
        let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
        // SAFETY: a V2 record of this process; the call takes the record's
        // address cast to `rusage_info_t *` (Apple's typedef of void *, so
        // the parameter reads as a pointer to a pointer but is the struct)
        let rc = unsafe {
            libc::proc_pid_rusage(
                std::process::id() as libc::c_int,
                libc::RUSAGE_INFO_V2,
                &mut info as *mut libc::rusage_info_v2 as *mut libc::rusage_info_t,
            )
        };
        return (rc == 0).then_some(info.ri_phys_footprint);
    }
    #[allow(unreachable_code)]
    None
}

/// the host's physical memory: Linux MemTotal, macOS hw.memsize
pub fn mem_total_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string("/proc/meminfo").ok()?;
        return s
            .lines()
            .find_map(|l| l.strip_prefix("MemTotal:"))
            .and_then(|rest| rest.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
            .map(|k| k * 1024);
    }
    #[cfg(target_os = "macos")]
    {
        let mut v: u64 = 0;
        let mut len = std::mem::size_of::<u64>();
        // SAFETY: hw.memsize is a u64 and the lengths say so
        let rc = unsafe {
            libc::sysctlbyname(
                c"hw.memsize".as_ptr(),
                &mut v as *mut u64 as *mut libc::c_void,
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        return (rc == 0 && v > 0).then_some(v);
    }
    #[allow(unreachable_code)]
    None
}

/// the resident size a build stops at
pub struct Ceiling {
    pub bytes: u64,
    /// where the number came from, for the stop line
    pub how: String,
}

/// `FLOE_RSS_CEILING`: unset = 90 % of MemTotal; a percent of MemTotal
/// (`80`, `80%`); bytes with K/M/G/T (`300G`); `off`/`0` = none. A
/// percent needs the host's memory: without it there is no ceiling.
pub fn parse_ceiling(spec: Option<&str>, total: Option<u64>) -> Result<Option<Ceiling>, String> {
    let percent = |p: f64, how: String| -> Option<Ceiling> {
        total.map(|t| Ceiling {
            bytes: (t as f64 * p / 100.0) as u64,
            how,
        })
    };
    let Some(raw) = spec else {
        return Ok(percent(
            90.0,
            format!("90% of MemTotal {}", total.map_or("?".to_string(), crate::fmt_size)),
        ));
    };
    let s = raw.trim();
    let lower = s.to_ascii_lowercase();
    if lower.is_empty() || lower == "off" || lower == "none" || lower == "0" {
        return Ok(None);
    }
    let split = lower
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(lower.len());
    let (num, unit) = lower.split_at(split);
    let bad = || {
        format!(
            "FLOE_RSS_CEILING={}: want a percent of MemTotal (e.g. 90), bytes with K/M/G/T (e.g. 300G), or off",
            s
        )
    };
    let value: f64 = num.parse().map_err(|_| bad())?;
    if !value.is_finite() || value <= 0.0 {
        return Err(bad());
    }
    let scale: u64 = match unit.trim() {
        "" | "%" => {
            if value > 100.0 {
                return Err(bad());
            }
            return Ok(percent(
                value,
                format!(
                    "FLOE_RSS_CEILING={}% of MemTotal {}",
                    value,
                    total.map_or("?".to_string(), crate::fmt_size)
                ),
            ));
        }
        "b" => 1,
        "k" | "kb" | "kib" => 1 << 10,
        "m" | "mb" | "mib" => 1 << 20,
        "g" | "gb" | "gib" => 1 << 30,
        "t" | "tb" | "tib" => 1 << 40,
        _ => return Err(bad()),
    };
    Ok(Some(Ceiling {
        bytes: (value * scale as f64) as u64,
        how: format!("FLOE_RSS_CEILING={}", s),
    }))
}

/// the ceiling of this run, decided once (a bad setting says so once and
/// the default applies)
pub fn ceiling() -> Option<&'static Ceiling> {
    static CEILING: std::sync::OnceLock<Option<Ceiling>> = std::sync::OnceLock::new();
    CEILING
        .get_or_init(|| {
            let total = mem_total_bytes();
            let spec = std::env::var("FLOE_RSS_CEILING").ok();
            match parse_ceiling(spec.as_deref(), total) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[vfs] {}: the default ceiling applies", e);
                    parse_ceiling(None, total).unwrap_or(None)
                }
            }
        })
        .as_ref()
}

static STREAMING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// the streaming build sets this so a stop advises the budget, not --stream
pub fn set_streaming(on: bool) {
    STREAMING.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// stop the process with one line when its anonymous resident size is
/// over the ceiling: `stage` names where the build was, `state` what the
/// cache is left as (CACHE_*); the advice follows the build's mode
pub fn check(stage: &str, state: &str) {
    let Some(c) = ceiling() else { return };
    let Some(rss) = rss_anon_bytes() else { return };
    if rss <= c.bytes {
        return;
    }
    let advice = if STREAMING.load(std::sync::atomic::Ordering::Relaxed) {
        ADVICE_BUDGET
    } else {
        ADVICE_STREAM
    };
    eprintln!(
        "[vfs] memory ceiling: rss {} is over {} ({}) during the {}: stopping before the \
         host swaps ({}); {}, or set FLOE_RSS_CEILING=<percent of MemTotal>|<bytes, e.g. 300G>|off",
        crate::fmt_size(rss),
        crate::fmt_size(c.bytes),
        c.how,
        stage,
        state,
        advice
    );
    std::process::exit(EXIT_MEMORY);
}

/// the in-memory build's size pre-check, before the file is read: a file
/// larger than MemAvailable cannot be read, let alone parsed (the parsed
/// form is 5-20x the file; `floe-index scan` sizes it) - refused with
/// EXIT_MEMORY and the cache untouched; a file over a fifth of
/// MemAvailable gets a note. Inert without MemAvailable (not Linux and no
/// FLOE_MEM_AVAILABLE_GB).
pub fn precheck_in_memory(src: &str) {
    let Ok(meta) = std::fs::metadata(src) else { return };
    let size = meta.len();
    let Some(avail) = crate::mem_available_gb().map(|g| (g * 1e9) as u64) else { return };
    if size > avail {
        eprintln!(
            "[vfs] {} is {} and MemAvailable is {}: the in-memory build reads the whole file and \
             parses it to several times its size - refused before the cache is touched (exit {}); {}",
            src,
            crate::fmt_size(size),
            crate::fmt_size(avail),
            EXIT_MEMORY,
            ADVICE_STREAM
        );
        std::process::exit(EXIT_MEMORY);
    }
    if size.saturating_mul(5) > avail {
        eprintln!(
            "[vfs] note: {} is {} against MemAvailable {}; the parsed form is 5-20x the file \
             (floe-index scan sizes it) - if this build stops at the memory ceiling, {}",
            src,
            crate::fmt_size(size),
            crate::fmt_size(avail),
            ADVICE_STREAM
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: u64 = 100 << 30;

    #[test]
    fn ceiling_spellings() {
        let c = parse_ceiling(None, Some(T)).unwrap().expect("default");
        assert_eq!(c.bytes, 90 << 30);
        assert!(c.how.starts_with("90% of MemTotal"));
        assert!(parse_ceiling(None, None).unwrap().is_none(), "no MemTotal: no default ceiling");
        for off in ["off", "OFF", "0", "none", "", "  off "] {
            assert!(parse_ceiling(Some(off), Some(T)).unwrap().is_none(), "{:?}", off);
        }
        assert_eq!(parse_ceiling(Some("80"), Some(T)).unwrap().unwrap().bytes, 80 << 30);
        assert_eq!(parse_ceiling(Some("50%"), Some(T)).unwrap().unwrap().bytes, 50 << 30);
        assert!(parse_ceiling(Some("80"), None).unwrap().is_none(), "a percent needs MemTotal");
        assert_eq!(parse_ceiling(Some("300G"), None).unwrap().unwrap().bytes, 300 << 30);
        assert_eq!(parse_ceiling(Some("1.5g"), Some(T)).unwrap().unwrap().bytes, 3 << 29);
        assert_eq!(parse_ceiling(Some("64M"), Some(T)).unwrap().unwrap().bytes, 64 << 20);
        assert_eq!(parse_ceiling(Some("2 TiB"), Some(T)).unwrap().unwrap().bytes, 2 << 40);
        assert_eq!(parse_ceiling(Some("4096k"), Some(T)).unwrap().unwrap().bytes, 4 << 20);
        for bad in ["abc", "-5", "101", "12x", "%", "1e3G"] {
            assert!(parse_ceiling(Some(bad), Some(T)).is_err(), "{:?} must be refused", bad);
        }
    }

    #[test]
    fn host_numbers_are_sane_where_known() {
        // Linux and macOS answer; elsewhere both are None and the guard is inert
        if let Some(total) = mem_total_bytes() {
            assert!(total > 1 << 28, "MemTotal {}", total);
            let rss = rss_anon_bytes().expect("rss where MemTotal is known");
            assert!(rss > 0 && rss < total, "rss {} of {}", rss, total);
        } else {
            assert!(rss_anon_bytes().is_none());
        }
    }
}
