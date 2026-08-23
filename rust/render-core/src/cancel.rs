use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Monotonic generation cancellation shared by a daemon command thread and
/// renderer workers. A generation is cancelled exactly when
/// `generation < before_generation`.
#[derive(Clone, Debug, Default)]
pub struct RenderCancellation {
    before_generation: Arc<AtomicU64>,
    commit_lock: Arc<Mutex<()>>,
}

impl RenderCancellation {
    pub fn new() -> Self {
        Self::default()
    }

    /// Raises the cancellation frontier. Older or equal frontiers are no-ops.
    pub fn cancel_before(&self, generation: u64) -> u64 {
        let _commit = self
            .commit_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.before_generation
            .fetch_max(generation, Ordering::AcqRel)
            .max(generation)
    }

    pub fn before_generation(&self) -> u64 {
        self.before_generation.load(Ordering::Acquire)
    }

    pub fn is_cancelled(&self, generation: u64) -> bool {
        generation < self.before_generation()
    }

    /// Runs a final publication step only while `generation` is current.
    /// Frontier changes and this closure are serialized; ordinary render
    /// cancellation checks remain lock-free.
    pub fn commit_if_current<T>(
        &self,
        generation: u64,
        commit: impl FnOnce() -> Result<T, String>,
    ) -> Result<Option<T>, String> {
        let _commit = self
            .commit_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.is_cancelled(generation) {
            Ok(None)
        } else {
            commit().map(Some)
        }
    }

    pub(crate) fn check(&self, generation: u64) -> Result<(), String> {
        if self.is_cancelled(generation) {
            Err(format!(
                "render cancelled: generation {} is before {}",
                generation,
                self.before_generation()
            ))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_frontier_is_monotonic_and_strict() {
        let cancellation = RenderCancellation::new();
        assert!(!cancellation.is_cancelled(0));
        assert_eq!(cancellation.cancel_before(8), 8);
        assert!(cancellation.is_cancelled(7));
        assert!(!cancellation.is_cancelled(8));
        assert_eq!(cancellation.cancel_before(3), 8);
        assert_eq!(cancellation.before_generation(), 8);

        let shared = cancellation.clone();
        shared.cancel_before(10);
        assert_eq!(cancellation.before_generation(), 10);
    }

    #[test]
    fn commit_is_skipped_for_a_stale_generation() {
        let cancellation = RenderCancellation::new();
        cancellation.cancel_before(5);
        let mut called = false;
        let stale = cancellation
            .commit_if_current(4, || {
                called = true;
                Ok(())
            })
            .unwrap();
        assert_eq!(stale, None);
        assert!(!called);

        let current = cancellation
            .commit_if_current(5, || {
                called = true;
                Ok(7)
            })
            .unwrap();
        assert_eq!(current, Some(7));
        assert!(called);
    }
}
