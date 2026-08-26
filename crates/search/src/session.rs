use crate::{SearchIndex, SearchStreamBatch, ShardedSearchIndex};
use gfm_jobs::Cancellation;
use gfm_types::{Result, SearchHit};
use std::sync::Mutex;

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
        let mut active = self
            .active
            .lock()
            .expect("search supersession lock poisoned");
        if let Some(previous) = active.replace(next.clone()) {
            previous.cancel();
        }
        next
    }

    pub fn cancel_active(&self) {
        let mut active = self
            .active
            .lock()
            .expect("search supersession lock poisoned");
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
}
