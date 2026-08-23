use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::{Cache, DecodedPage, RenderStats};

struct Entry {
    page: Arc<DecodedPage>,
    charge: u64,
    last_used: u64,
}

/// Budgeted immutable decoded-page cache.
///
/// Eviction is deterministic: least-recently-used first, then lowest page id.
/// A page larger than the entire budget is returned to the caller but is not
/// retained by the cache.
pub struct DecodedPageCache {
    budget_bytes: u64,
    resident_bytes: u64,
    clock: u64,
    entries: HashMap<u32, Entry>,
}

impl DecodedPageCache {
    pub fn new(budget_bytes: u64) -> Self {
        Self {
            budget_bytes,
            resident_bytes: 0,
            clock: 0,
            entries: HashMap::new(),
        }
    }

    pub fn budget_bytes(&self) -> u64 {
        self.budget_bytes
    }

    pub fn resident_bytes(&self) -> u64 {
        self.resident_bytes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, page_id: u32) -> bool {
        self.entries.contains_key(&page_id)
    }

    pub fn set_budget_bytes(&mut self, budget_bytes: u64) {
        self.budget_bytes = budget_bytes;
        self.evict_to_fit(0);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.resident_bytes = 0;
    }

    pub fn get(&mut self, page_id: u32) -> Option<Arc<DecodedPage>> {
        self.clock = self.clock.saturating_add(1);
        let tick = self.clock;
        self.entries.get_mut(&page_id).map(|entry| {
            entry.last_used = tick;
            Arc::clone(&entry.page)
        })
    }

    pub fn insert(&mut self, page: Arc<DecodedPage>) -> bool {
        let page_id = page.page_id;
        self.clock = self.clock.saturating_add(1);
        let tick = self.clock;
        if let Some(entry) = self.entries.get_mut(&page_id) {
            entry.last_used = tick;
            return true;
        }

        let charge = page.estimated_bytes();
        if charge > self.budget_bytes {
            return false;
        }
        self.evict_to_fit(charge);
        self.resident_bytes = self.resident_bytes.saturating_add(charge);
        self.entries.insert(
            page_id,
            Entry {
                page,
                charge,
                last_used: tick,
            },
        );
        true
    }

    /// Resolves pages in caller order. Cache misses are deduplicated and read
    /// in one VFS batch; duplicate page ids share the same `Arc`.
    pub fn load(
        &mut self,
        source: &Cache,
        page_ids: &[u32],
    ) -> Result<(Vec<Arc<DecodedPage>>, RenderStats), String> {
        self.load_impl(source, page_ids, 1, None)
    }

    pub fn load_parallel(
        &mut self,
        source: &Cache,
        page_ids: &[u32],
        workers: u16,
    ) -> Result<(Vec<Arc<DecodedPage>>, RenderStats), String> {
        self.load_impl(source, page_ids, workers, None)
    }

    pub fn load_cancellable(
        &mut self,
        source: &Cache,
        page_ids: &[u32],
        workers: u16,
        generation: u64,
        cancellation: &crate::RenderCancellation,
    ) -> Result<(Vec<Arc<DecodedPage>>, RenderStats), String> {
        self.load_impl(source, page_ids, workers, Some((generation, cancellation)))
    }

    fn load_impl(
        &mut self,
        source: &Cache,
        page_ids: &[u32],
        workers: u16,
        guard: Option<(u64, &crate::RenderCancellation)>,
    ) -> Result<(Vec<Arc<DecodedPage>>, RenderStats), String> {
        let mut resolved: HashMap<u32, Arc<DecodedPage>> = HashMap::new();
        let mut missing = Vec::new();
        let mut missing_set = HashSet::new();
        let mut cache_hits = 0u32;

        for &page_id in page_ids {
            if let Some(page) = self.get(page_id) {
                resolved.insert(page_id, page);
                cache_hits = cache_hits.saturating_add(1);
            } else if missing_set.insert(page_id) {
                missing.push(page_id);
            }
        }

        let (decoded, mut stats) = match guard {
            Some((generation, cancellation)) => {
                source.decode_pages_cancellable(&missing, workers, generation, cancellation)?
            }
            None => source.decode_pages_parallel(&missing, workers)?,
        };
        for page in decoded {
            check_load_cancelled(guard)?;
            let page = Arc::new(page);
            resolved.insert(page.page_id, Arc::clone(&page));
            self.insert(page);
        }
        check_load_cancelled(guard)?;
        stats.decoded_cache_hit = cache_hits;
        stats.decoded_cache_miss = missing.len().try_into().unwrap_or(u32::MAX);
        stats.decoded_cache_bytes = self.resident_bytes;

        let mut pages = Vec::with_capacity(page_ids.len());
        for &page_id in page_ids {
            let page = resolved
                .get(&page_id)
                .ok_or_else(|| format!("internal error: unresolved page {}", page_id))?;
            pages.push(Arc::clone(page));
        }
        Ok((pages, stats))
    }

    fn evict_to_fit(&mut self, incoming: u64) {
        while self.resident_bytes.saturating_add(incoming) > self.budget_bytes {
            let victim = self
                .entries
                .iter()
                .min_by_key(|(page_id, entry)| (entry.last_used, **page_id))
                .map(|(page_id, _)| *page_id);
            let Some(victim) = victim else {
                break;
            };
            if let Some(entry) = self.entries.remove(&victim) {
                self.resident_bytes = self.resident_bytes.saturating_sub(entry.charge);
            }
        }
    }
}

fn check_load_cancelled(guard: Option<(u64, &crate::RenderCancellation)>) -> Result<(), String> {
    if let Some((generation, cancellation)) = guard {
        if cancellation.is_cancelled(generation) {
            return Err(format!(
                "render cancelled: generation {} is before {}",
                generation,
                cancellation.before_generation()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use floe_oasis::doc::{Cell, Doc};
    use floe_ovm::BBox;

    fn page(page_id: u32) -> Arc<DecodedPage> {
        Arc::new(DecodedPage {
            page_id,
            layer_idx: 0,
            bbox: BBox {
                x0: 0,
                y0: 0,
                x1: 10,
                y1: 10,
            },
            encoded_bytes: 1,
            records: 0,
            members: 0,
            doc: Doc {
                unit: 1000.0,
                cells: vec![Cell::default()],
                top: 0,
                layer_order: Vec::new(),
                norm_s: 0.0,
                layer_names: HashMap::new(),
                layer_aliases: HashMap::new(),
            },
        })
    }

    #[test]
    fn evicts_lru_with_page_id_tie_break() {
        let p0 = page(0);
        let p1 = page(1);
        let p2 = page(2);
        let charge = p0.estimated_bytes();
        let mut cache = DecodedPageCache::new(charge * 2);
        assert!(cache.insert(p0));
        assert!(cache.insert(p1));
        assert!(cache.get(0).is_some());
        assert!(cache.insert(p2));
        assert!(cache.contains(0));
        assert!(!cache.contains(1));
        assert!(cache.contains(2));
        assert!(cache.resident_bytes() <= cache.budget_bytes());
    }

    #[test]
    fn oversized_page_is_not_retained() {
        let page = page(9);
        let mut cache = DecodedPageCache::new(page.estimated_bytes() - 1);
        assert!(!cache.insert(page));
        assert!(cache.is_empty());
        assert_eq!(cache.resident_bytes(), 0);
    }
}
