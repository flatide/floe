use std::collections::{BTreeMap, HashMap, HashSet};
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
    /// Cumulative eviction count; loads report their delta.
    evictions: u64,
    entries: HashMap<u32, Entry>,
    /// `(last_used, page_id)` mirror of `entries` so the eviction
    /// victim is an O(log n) first-key lookup. The full-map
    /// `min_by_key` scan it replaces cost O(misses x resident) per
    /// load - a long session at a full budget paid seconds for a
    /// minimap jump that a fresh viewer served in 200ms (§3.18).
    lru: BTreeMap<(u64, u32), ()>,
}

impl DecodedPageCache {
    pub fn new(budget_bytes: u64) -> Self {
        Self {
            budget_bytes,
            resident_bytes: 0,
            clock: 0,
            evictions: 0,
            entries: HashMap::new(),
            lru: BTreeMap::new(),
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
        self.lru.clear();
        self.resident_bytes = 0;
    }

    pub fn get(&mut self, page_id: u32) -> Option<Arc<DecodedPage>> {
        self.clock = self.clock.saturating_add(1);
        let tick = self.clock;
        let lru = &mut self.lru;
        self.entries.get_mut(&page_id).map(|entry| {
            lru.remove(&(entry.last_used, page_id));
            lru.insert((tick, page_id), ());
            entry.last_used = tick;
            Arc::clone(&entry.page)
        })
    }

    pub fn insert(&mut self, page: Arc<DecodedPage>) -> bool {
        let page_id = page.page_id;
        self.clock = self.clock.saturating_add(1);
        let tick = self.clock;
        if let Some(entry) = self.entries.get_mut(&page_id) {
            self.lru.remove(&(entry.last_used, page_id));
            self.lru.insert((tick, page_id), ());
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
        self.lru.insert((tick, page_id), ());
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

        let evictions_before = self.evictions;
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
        stats.decoded_cache_evicted = self
            .evictions
            .saturating_sub(evictions_before)
            .try_into()
            .unwrap_or(u32::MAX);
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
            // Same deterministic order as the old full scan:
            // least-recently-used first, then lowest page id.
            let Some((&(last_used, victim), ())) = self.lru.first_key_value() else {
                break;
            };
            self.lru.remove(&(last_used, victim));
            if let Some(entry) = self.entries.remove(&victim) {
                self.resident_bytes = self.resident_bytes.saturating_sub(entry.charge);
                self.evictions = self.evictions.saturating_add(1);
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
        let doc = Doc {
            unit: 1000.0,
            cells: vec![Cell::default()],
            top: 0,
            layer_order: Vec::new(),
            norm_s: 0.0,
            layer_names: HashMap::new(),
            layer_aliases: HashMap::new(),
        };
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
            index: crate::PageIndex::build(&doc),
            doc,
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

    #[test]
    fn lru_index_stays_consistent_under_touch_and_churn() {
        // The O(log n) lru mirror (§3.18) must agree with the entry
        // map through interleaved touches, re-inserts, evictions, and
        // budget shrinks - the victim order is (last_used, page_id),
        // exactly like the full scan it replaced.
        let charge = page(0).estimated_bytes();
        let mut cache = DecodedPageCache::new(charge * 8);
        for id in 0..8 {
            assert!(cache.insert(page(id)));
        }
        assert_eq!(cache.lru.len(), cache.entries.len());
        // Touch a spread of pages (get) and re-insert one (insert on a
        // resident page must retouch, not duplicate).
        for &id in &[0u32, 2, 4, 6, 0, 2] {
            assert!(cache.get(id).is_some());
        }
        assert!(cache.insert(page(4)));
        assert_eq!(cache.lru.len(), cache.entries.len());
        // Untouched pages leave first, oldest touch next.
        cache.set_budget_bytes(charge * 4);
        for id in [1u32, 3, 5, 7] {
            assert!(!cache.contains(id), "untouched page {id} must evict");
        }
        cache.set_budget_bytes(charge * 2);
        assert!(!cache.contains(6), "oldest touch must evict next");
        assert!(!cache.contains(0));
        assert!(cache.contains(2));
        assert!(cache.contains(4), "re-inserted page is the newest");
        assert_eq!(cache.lru.len(), cache.entries.len());
        assert_eq!(cache.resident_bytes(), charge * 2);
        // Churn through fresh ids at the tight budget: the index must
        // track every insert/evict pair.
        for id in 100..200 {
            assert!(cache.insert(page(id)));
            assert_eq!(cache.lru.len(), cache.entries.len());
            assert!(cache.resident_bytes() <= cache.budget_bytes());
        }
        assert_eq!(cache.entries.len(), 2);
        assert!(cache.contains(198) && cache.contains(199));
    }
}
