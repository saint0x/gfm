use crate::{SearchIndex, SearchStreamBatch, ShardedSearchIndex};
use gfm_jobs::Cancellation;
use gfm_types::{Result, SearchHit};
use std::sync::{Mutex, MutexGuard};

#[derive(Debug, Default)]
pub struct SearchSupersession {
    active: Mutex<Option<Cancellation>>,
}

impl SearchSupersession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn begin(&self) -> Cancellation {
        let next = Cancellation::default();
        let mut active = self.active_lock();
        if let Some(previous) = active.replace(next.clone()) {
            previous.cancel();
        }
        next
    }

    pub fn cancel_active(&self) {
        let mut active = self.active_lock();
        if let Some(previous) = active.take() {
            previous.cancel();
        }
    }

    pub fn query(&self, index: &SearchIndex, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let cancellation = self.begin();
        index.query_cancellable(query, limit, &cancellation)
    }

    pub fn query_sharded(
        &self,
        index: &ShardedSearchIndex,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let cancellation = self.begin();
        index.query_cancellable(query, limit, &cancellation)
    }

    pub fn stream(
        &self,
        index: &SearchIndex,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchStreamBatch>> {
        let cancellation = self.begin();
        index.stream_cancellable(query, limit, &cancellation)
    }

    pub fn stream_sharded(
        &self,
        index: &ShardedSearchIndex,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchStreamBatch>> {
        let cancellation = self.begin();
        index.stream_cancellable(query, limit, &cancellation)
    }

    fn active_lock(&self) -> MutexGuard<'_, Option<Cancellation>> {
        self.active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_types::GfmError;
    use std::panic::{self, AssertUnwindSafe};

    #[test]
    fn begin_recovers_poisoned_active_lock() {
        let supersession = SearchSupersession::new();
        let first = supersession.begin();

        poison_active_lock(&supersession);
        let second = supersession.begin();

        assert!(matches!(first.check(), Err(GfmError::Cancelled)));
        assert!(second.check().is_ok());
    }

    #[test]
    fn cancel_active_recovers_poisoned_active_lock() {
        let supersession = SearchSupersession::new();
        let active = supersession.begin();

        poison_active_lock(&supersession);
        supersession.cancel_active();

        assert!(matches!(active.check(), Err(GfmError::Cancelled)));
    }

    fn poison_active_lock(supersession: &SearchSupersession) {
        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            let _guard = supersession
                .active
                .lock()
                .expect("initial supersession lock");
            panic!("poison search supersession lock");
        }));
    }
}
