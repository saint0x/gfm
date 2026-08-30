use crate::{SearchIndex, SearchStreamBatch, SearchVolumeScope, ShardedSearchIndex};
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
        let query = crate::SearchQuery::parse_cancellable(query, &cancellation)?;
        index.query_structured_cancellable(&query, limit, &cancellation)
    }

    pub fn query_structured(
        &self,
        index: &SearchIndex,
        query: &crate::SearchQuery,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let cancellation = self.begin();
        index.query_structured_cancellable(query, limit, &cancellation)
    }

    pub fn query_sharded(
        &self,
        index: &ShardedSearchIndex,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let cancellation = self.begin();
        let query = crate::SearchQuery::parse_cancellable(query, &cancellation)?;
        index.query_structured_cancellable(&query, limit, &cancellation)
    }

    pub fn query_sharded_structured(
        &self,
        index: &ShardedSearchIndex,
        query: &crate::SearchQuery,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let cancellation = self.begin();
        index.query_structured_cancellable(query, limit, &cancellation)
    }

    pub fn query_sharded_with_volume_scope(
        &self,
        index: &ShardedSearchIndex,
        query: &str,
        limit: usize,
        scope: &SearchVolumeScope,
    ) -> Result<Vec<SearchHit>> {
        let cancellation = self.begin();
        let query = crate::SearchQuery::parse_cancellable(query, &cancellation)?;
        index.query_structured_with_volume_scope_cancellable(&query, limit, scope, &cancellation)
    }

    pub fn query_sharded_structured_with_volume_scope(
        &self,
        index: &ShardedSearchIndex,
        query: &crate::SearchQuery,
        limit: usize,
        scope: &SearchVolumeScope,
    ) -> Result<Vec<SearchHit>> {
        let cancellation = self.begin();
        index.query_structured_with_volume_scope_cancellable(query, limit, scope, &cancellation)
    }

    pub fn stream(
        &self,
        index: &SearchIndex,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchStreamBatch>> {
        let cancellation = self.begin();
        let query = crate::SearchQuery::parse_cancellable(query, &cancellation)?;
        index.stream_structured_cancellable(&query, limit, &cancellation)
    }

    pub fn stream_structured(
        &self,
        index: &SearchIndex,
        query: &crate::SearchQuery,
        limit: usize,
    ) -> Result<Vec<SearchStreamBatch>> {
        let cancellation = self.begin();
        index.stream_structured_cancellable(query, limit, &cancellation)
    }

    pub fn stream_sharded(
        &self,
        index: &ShardedSearchIndex,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchStreamBatch>> {
        let cancellation = self.begin();
        let query = crate::SearchQuery::parse_cancellable(query, &cancellation)?;
        index.stream_structured_cancellable(&query, limit, &cancellation)
    }

    pub fn stream_sharded_structured(
        &self,
        index: &ShardedSearchIndex,
        query: &crate::SearchQuery,
        limit: usize,
    ) -> Result<Vec<SearchStreamBatch>> {
        let cancellation = self.begin();
        index.stream_structured_cancellable(query, limit, &cancellation)
    }

    pub fn stream_sharded_with_volume_scope(
        &self,
        index: &ShardedSearchIndex,
        query: &str,
        limit: usize,
        scope: &SearchVolumeScope,
    ) -> Result<Vec<SearchStreamBatch>> {
        let cancellation = self.begin();
        let query = crate::SearchQuery::parse_cancellable(query, &cancellation)?;
        index.stream_structured_with_volume_scope_cancellable(&query, limit, scope, &cancellation)
    }

    pub fn stream_sharded_structured_with_volume_scope(
        &self,
        index: &ShardedSearchIndex,
        query: &crate::SearchQuery,
        limit: usize,
        scope: &SearchVolumeScope,
    ) -> Result<Vec<SearchStreamBatch>> {
        let cancellation = self.begin();
        index.stream_structured_with_volume_scope_cancellable(query, limit, scope, &cancellation)
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
    use gfm_types::{FileId, FileKind, GfmError, VolumeId};
    use std::panic::{self, AssertUnwindSafe};
    use std::path::PathBuf;

    #[test]
    fn query_sharded_structured_with_volume_scope_preserves_filters() {
        let mut index = ShardedSearchIndex::new();
        let mut directory = record(FileId::new(VolumeId(1), 1), "/Volumes/A/report", "report");
        directory.kind = FileKind::Directory;
        index.insert(directory);
        index.insert(record(
            FileId::new(VolumeId(2), 2),
            "/Volumes/B/report.md",
            "report.md",
        ));
        let supersession = SearchSupersession::new();
        let query = crate::SearchQuery::parse("report kind:file");

        let hits = supersession
            .query_sharded_structured_with_volume_scope(
                &index,
                &query,
                10,
                &SearchVolumeScope::only([VolumeId(2)]),
            )
            .unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record.kind, FileKind::File);
        assert_eq!(hits[0].record.path, PathBuf::from("/Volumes/B/report.md"));
    }

    #[test]
    fn stream_sharded_structured_cancels_previous_query() {
        let mut index = ShardedSearchIndex::new();
        index.insert(record(
            FileId::new(VolumeId(1), 1),
            "/Volumes/A/needle.md",
            "needle.md",
        ));
        let supersession = SearchSupersession::new();
        let previous = supersession.begin();
        let query = crate::SearchQuery::parse("needle kind:file");

        let batches = supersession
            .stream_sharded_structured(&index, &query, 10)
            .unwrap();

        assert!(matches!(previous.check(), Err(GfmError::Cancelled)));
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].stage, crate::SearchStreamStage::Hot);
        assert_eq!(batches[0].hits[0].record.name, "needle.md");
    }

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

    fn record(id: FileId, path: &str, name: &str) -> gfm_types::FileRecord {
        gfm_types::FileRecord {
            id,
            parent: None,
            path: PathBuf::from(path),
            name: name.to_string(),
            kind: FileKind::File,
            len: 0,
            mode: 0,
            owner: 0,
            group: 0,
            xattrs_digest: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: Vec::new(),
            finder_comment: None,
        }
    }
}
