//! Process-local admission and all-or-nothing cache leases. These do not
//! constrain another gateway/legacy indexer or claim a hard RSS/thread cap.
use crate::{
    cache,
    dataset::Dataset,
    jobdeck::{color::Mode, index::is_deck, parser::JobDeck},
    render::RenderOptions,
    Error, ErrorKind, Result,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub cpu_slots: u32,
    /// Capacity never borrowed by index jobs; not a latency/preemption promise.
    pub foreground_reserve: u32,
    pub workers: u32,
    pub decoded_mb: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            cpu_slots: 16,
            foreground_reserve: 4,
            workers: 2,
            decoded_mb: 2048,
        }
    }
}
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Usage {
    pub cpu_slots: u32,
    pub workers: u32,
    pub decoded_mb: u64,
    pub index_jobs: u32,
}
#[derive(Default)]
struct Hold {
    readers: usize,
    writer: bool,
}
struct State {
    usage: Usage,
    leases: BTreeMap<PathBuf, Hold>,
    next_id: u64,
}
pub struct Resources {
    limits: Limits,
    state: Mutex<State>,
}
impl Resources {
    pub fn new(limits: Limits) -> Result<Arc<Self>> {
        if limits.cpu_slots == 0
            || limits.cpu_slots > 256
            || limits.foreground_reserve >= limits.cpu_slots
            || limits.workers == 0
            || limits.workers > 32
            || limits.decoded_mb == 0
            || limits.decoded_mb > (1 << 20)
        {
            return Err(Error::input("invalid managed resource limits"));
        }
        Ok(Arc::new(Self {
            limits,
            state: Mutex::new(State {
                usage: Usage::default(),
                leases: BTreeMap::new(),
                next_id: 0,
            }),
        }))
    }
    pub fn usage(&self) -> Usage {
        self.state.lock().unwrap().usage
    }
    /// Read-only UI advice, not a reservation or cache-writer preflight. Index
    /// admission rechecks the same limits after the user chooses their jobs.
    pub fn index_slots(&self) -> u32 {
        let usage = self.usage();
        if usage.index_jobs != 0 {
            return 0;
        }
        self.limits
            .cpu_slots
            .saturating_sub(usage.cpu_slots)
            .min(self.limits.cpu_slots - self.limits.foreground_reserve)
            .min(16)
    }
    pub fn next_id(&self) -> Result<u64> {
        let mut s = self.state.lock().unwrap();
        s.next_id = s
            .next_id
            .checked_add(1)
            .ok_or_else(|| Error::input("identity space exhausted"))?;
        Ok(s.next_id)
    }
    pub fn render(self: &Arc<Self>, options: &RenderOptions) -> Result<Permit> {
        if options.decode_jobs == 0 || options.raster_jobs == 0 || options.budget_mb == 0 {
            return Err(Error::input("invalid managed render reservation"));
        }
        // Conservatively reserve both pools, including mixed deck passes.
        // I/O/control threads, mmap, generation/retained masks are not this budget.
        let use_ = Usage {
            cpu_slots: u32::from(options.decode_jobs) + u32::from(options.raster_jobs),
            workers: 1,
            decoded_mb: options.budget_mb,
            index_jobs: 0,
        };
        self.acquire(use_, BTreeSet::new(), false)
    }
    pub fn read(self: &Arc<Self>, caches: impl IntoIterator<Item = PathBuf>) -> Result<Permit> {
        self.acquire(Usage::default(), keys(caches)?, false)
    }
    /// One bounded directory catalogue actor. This admission estimate includes
    /// sorting keys, opaque handle ancestry and response history, not a hard RSS
    /// ceiling or an NFS latency guarantee. No dataset lease is needed to list.
    pub fn browse(self: &Arc<Self>) -> Result<Permit> {
        self.acquire(
            Usage {
                cpu_slots: 1,
                decoded_mb: 192,
                ..Usage::default()
            },
            BTreeSet::new(),
            false,
        )
    }
    /// Dedicated exact export worker. The dataset's separate read permit must
    /// remain alive through child reap; this reserves CPU and decoded memory,
    /// not the native geometry/writer's total RSS or temporary disk footprint.
    pub fn export(self: &Arc<Self>, jobs: u16, budget_mb: u64) -> Result<Permit> {
        if !(1..=16).contains(&jobs) || budget_mb == 0 {
            return Err(Error::input("invalid managed export reservation"));
        }
        self.acquire(
            Usage {
                cpu_slots: u32::from(jobs),
                workers: 1,
                decoded_mb: budget_mb,
                index_jobs: 0,
            },
            BTreeSet::new(),
            false,
        )
    }
    /// One DRC reader owns metadata, a coordinate LRU and bounded replies.
    /// Reserve its CPU even while idle, like a render worker. decoded_mb is the
    /// existing shared read-memory admission pool, NOT a process RSS ceiling.
    pub fn drc(self: &Arc<Self>, files: impl IntoIterator<Item = PathBuf>) -> Result<Permit> {
        self.drc_with_rules(files, false)
    }
    /// Metadata parsing has its own bounded input/owned allocation peak. Keep
    /// its reservation separate from pack/LRU/replies; no extra CPU worker.
    pub fn drc_with_rules(
        self: &Arc<Self>,
        files: impl IntoIterator<Item = PathBuf>,
        rules: bool,
    ) -> Result<Permit> {
        self.acquire(
            Usage {
                cpu_slots: 1,
                decoded_mb: if rules { 512 } else { 256 },
                ..Usage::default()
            },
            keys(files)?,
            false,
        )
    }
    /// Hold this permit across prepare/start/poll/cancel AND child reap. The
    /// ordinary PreparedIndex still owns the cross-process exclusive OS lock.
    pub fn index(
        self: &Arc<Self>,
        caches: impl IntoIterator<Item = PathBuf>,
        jobs: usize,
    ) -> Result<Permit> {
        if !(1..=16).contains(&jobs) {
            return Err(Error::input("managed indexing requires 1..16 jobs"));
        }
        self.acquire(
            Usage {
                cpu_slots: jobs as u32,
                index_jobs: 1,
                ..Usage::default()
            },
            keys(caches)?,
            true,
        )
    }
    fn acquire(
        self: &Arc<Self>,
        use_: Usage,
        keys: BTreeSet<PathBuf>,
        write: bool,
    ) -> Result<Permit> {
        let mut s = self.state.lock().unwrap();
        let u = s.usage;
        // At most one index job is admitted. Limit that job's reservation,
        // not the total including foreground work already using its reserve.
        // Otherwise render->index and index->render have different outcomes.
        let index_borrows_reserve =
            write && use_.cpu_slots > self.limits.cpu_slots - self.limits.foreground_reserve;
        let leases_conflict = keys.iter().any(|key| {
            s.leases
                .get(key)
                .is_some_and(|h| h.writer || (write && h.readers > 0))
        });
        if u.cpu_slots + use_.cpu_slots > self.limits.cpu_slots
            || index_borrows_reserve
            || u.workers + use_.workers > self.limits.workers
            || use_.decoded_mb > self.limits.decoded_mb.saturating_sub(u.decoded_mb)
            || u.index_jobs + use_.index_jobs > 1
            || leases_conflict
        {
            return Err(Error::new(
                ErrorKind::Busy,
                "managed resource or cache lease is busy",
            ));
        }
        for key in &keys {
            let h = s.leases.entry(key.clone()).or_default();
            if write {
                h.writer = true;
            } else {
                h.readers += 1;
            }
        }
        s.usage = Usage {
            cpu_slots: u.cpu_slots + use_.cpu_slots,
            workers: u.workers + use_.workers,
            decoded_mb: u.decoded_mb + use_.decoded_mb,
            index_jobs: u.index_jobs + use_.index_jobs,
        };
        Ok(Permit {
            resources: Arc::clone(self),
            usage: use_,
            keys,
            write,
        })
    }
}
pub struct Permit {
    resources: Arc<Resources>,
    usage: Usage,
    keys: BTreeSet<PathBuf>,
    write: bool,
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut s = self.resources.state.lock().unwrap();
        s.usage.cpu_slots -= self.usage.cpu_slots;
        s.usage.workers -= self.usage.workers;
        s.usage.decoded_mb -= self.usage.decoded_mb;
        s.usage.index_jobs -= self.usage.index_jobs;
        for key in &self.keys {
            let h = s.leases.get_mut(key).expect("owned lease");
            if self.write {
                h.writer = false;
            } else {
                h.readers -= 1;
            }
            if !h.writer && h.readers == 0 {
                s.leases.remove(key);
            }
        }
    }
}
fn keys(paths: impl IntoIterator<Item = PathBuf>) -> Result<BTreeSet<PathBuf>> {
    let mut keys = BTreeSet::new();
    for path in paths {
        let path = cache::absolute(&path)?;
        // Resolve parent aliases even when the destination cache does not yet
        // exist; never key the lock by a symlink spelling alone.
        let mut ancestor = path.as_path();
        let mut missing = Vec::new();
        let key = loop {
            match fs::canonicalize(ancestor) {
                Ok(mut p) => {
                    for name in missing.iter().rev() {
                        p.push(name);
                    }
                    break p;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Missing deck TCs still participate in the lease set,
                    // even when an intermediate source directory is absent.
                    missing.push(
                        ancestor
                            .file_name()
                            .ok_or_else(|| Error::input("cache has no name"))?,
                    );
                    ancestor = ancestor
                        .parent()
                        .ok_or_else(|| Error::input("cache has no ancestor"))?;
                }
                Err(e) => return Err(e.into()),
            }
        };
        keys.insert(key);
        if keys.len() > 65536 {
            return Err(Error::input("managed cache lease set exceeds 65536"));
        }
    }
    Ok(keys)
}

pub struct ManagedDataset {
    pub dataset: Dataset,
    /// Unique within Resources lifetime. Web also binds this to its server/view epoch.
    pub revision: u64,
    _read: Permit,
}
impl ManagedDataset {
    /// Trusted local registration only. Browser input must resolve a previously
    /// authorized dataset handle, not be passed here as a filesystem path.
    pub fn open(
        resources: &Arc<Resources>,
        source: &Path,
        levels: Option<BTreeSet<i64>>,
        mode: Mode,
        cancelled: &AtomicUsize,
    ) -> Result<Arc<Self>> {
        crate::check_cancelled(cancelled)?;
        let source = cache::absolute(source)?;
        let caches = if is_deck(&source) {
            let deck = JobDeck::read(&source, true, cancelled)?;
            let selected = levels.as_ref().filter(|s| !s.is_empty());
            crate::jobdeck::index::validate_levels(&deck, selected)?;
            deck.sources(selected)
                .into_iter()
                .map(|tc| cache::cache_path(&source.parent().unwrap().join(tc)))
                .collect::<Result<Vec<_>>>()?
        } else {
            vec![cache::cache_path(&source)?]
        };
        let read = resources.read(caches)?;
        let dataset = Dataset::open(&source, levels, mode, cancelled)?;
        let actual = match &dataset {
            Dataset::Layout(l) => keys([l.directory.clone()])?,
            Dataset::Deck(d) => keys(
                d.analysis
                    .catalog
                    .infos
                    .values()
                    .map(|i| cache::cache_path(&i.path))
                    .collect::<Result<Vec<_>>>()?,
            )?,
        };
        if actual != read.keys {
            return Err(Error::new(
                ErrorKind::Cache,
                "dataset source set changed during open",
            ));
        }
        if cancelled.load(Ordering::Relaxed) != 0 {
            return Err(Error::new(ErrorKind::Cancelled, "open cancelled"));
        }
        Ok(Arc::new(Self {
            dataset,
            revision: resources.next_id()?,
            _read: read,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn index_slot_advice_obeys_foreground_reserve_without_reserving() {
        let r = Resources::new(Limits::default()).unwrap();
        assert_eq!(r.index_slots(), 12);
        let browse = r.browse().unwrap();
        let render = r
            .render(&RenderOptions {
                decode_jobs: 8,
                raster_jobs: 4,
                budget_mb: 64,
                binary: PathBuf::from("unused-index-slot-test-renderer"),
                tile_px: 384,
                round_pages: 1024,
                open_timeout_s: 30,
                label_font_px: 14,
                raw: true,
            })
            .unwrap();
        let before = r.usage();
        assert_eq!(r.index_slots(), 3);
        assert_eq!(r.usage(), before);
        let index = r.index(Vec::new(), 3).unwrap();
        assert_eq!(r.index_slots(), 0);
        drop(index);
        assert_eq!(r.index_slots(), 3);
        drop((browse, render));
        assert_eq!(r.index_slots(), 12);
    }
    #[test]
    fn drc_read_reservation_is_symmetric_and_shared_with_render() {
        let r = Resources::new(Limits::default()).unwrap();
        let drc = r.drc([]).unwrap();
        assert_eq!(r.usage().cpu_slots, 1);
        assert_eq!(r.usage().decoded_mb, 256);
        assert_eq!(r.usage().workers, 0);
        assert!(r
            .acquire(
                Usage {
                    cpu_slots: 2,
                    workers: 1,
                    decoded_mb: 2048,
                    ..Usage::default()
                },
                BTreeSet::new(),
                false
            )
            .is_err());
        drop(drc);
        assert_eq!(r.usage(), Usage::default());
        let with_rules = r.drc_with_rules([], true).unwrap();
        assert_eq!(r.usage().decoded_mb, 512);
        assert_eq!(r.usage().cpu_slots, 1);
        assert_eq!(r.usage().workers, 0);
        assert!(r
            .acquire(
                Usage {
                    decoded_mb: 1537,
                    ..Usage::default()
                },
                BTreeSet::new(),
                false
            )
            .is_err());
        let fits = r
            .acquire(
                Usage {
                    decoded_mb: 1536,
                    ..Usage::default()
                },
                BTreeSet::new(),
                false,
            )
            .unwrap();
        drop((fits, with_rules));
        assert_eq!(r.usage(), Usage::default());
    }
    #[test]
    fn leases_are_atomic_and_aliases_share_identity() {
        let dir = std::env::temp_dir().join(format!("floe-managed-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        std::os::unix::fs::symlink(&dir, dir.join("alias")).unwrap();
        let r = Resources::new(Limits::default()).unwrap();
        let a = dir.join("a.floe");
        let b = dir.join("b.floe");
        let read = r.read([a.clone()]).unwrap();
        assert!(r.index([dir.join("alias/a.floe"), b.clone()], 4).is_err());
        assert_eq!(r.usage(), Usage::default());
        let other = r.index([b.clone()], 12).unwrap();
        assert!(r.read([b.clone()]).is_err());
        assert!(r.read([a.clone(), b]).is_err());
        drop(other);
        drop(read);
        let write = r.index([a], 12).unwrap();
        drop(write);
        assert_eq!(r.usage(), Usage::default());
        fs::remove_file(dir.join("alias")).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn quotas_rollback_and_keep_foreground_capacity() {
        let r = Resources::new(Limits::default()).unwrap();
        assert!(r.index([], 17).is_err());
        assert!(r.index([], 16).is_err());
        let index = r.index([], 12).unwrap();
        assert!(r.index([], 1).is_err());
        let render = r
            .acquire(
                Usage {
                    cpu_slots: 4,
                    workers: 1,
                    decoded_mb: 1024,
                    index_jobs: 0,
                },
                BTreeSet::new(),
                false,
            )
            .unwrap();
        assert!(r
            .acquire(
                Usage {
                    cpu_slots: 1,
                    ..Usage::default()
                },
                BTreeSet::new(),
                false
            )
            .is_err());
        assert_eq!(r.usage().cpu_slots, 16);
        drop(index);
        drop(render);
        assert_eq!(r.usage(), Usage::default());
        assert_ne!(r.next_id().unwrap(), r.next_id().unwrap());
    }
    #[test]
    fn foreground_reserve_is_independent_of_admission_order() {
        for (foreground, indexing) in [(4, 12), (12, 4)] {
            for index_first in [true, false] {
                let r = Resources::new(Limits::default()).unwrap();
                let render = || {
                    r.acquire(
                        Usage {
                            cpu_slots: foreground,
                            workers: 1,
                            ..Usage::default()
                        },
                        BTreeSet::new(),
                        false,
                    )
                    .unwrap()
                };
                let index = || r.index([], indexing).unwrap();
                let (a, b) = if index_first {
                    (index(), render())
                } else {
                    (render(), index())
                };
                assert_eq!(r.usage().cpu_slots, 16);
                assert!(r.index([], 1).is_err());
                drop((a, b));
                assert_eq!(r.usage(), Usage::default());
            }
        }
        let r = Resources::new(Limits {
            foreground_reserve: 0,
            ..Limits::default()
        })
        .unwrap();
        let index = r.index([], 16).unwrap();
        assert_eq!(r.usage().cpu_slots, 16);
        drop(index);
        assert_eq!(r.usage(), Usage::default());
    }
}
