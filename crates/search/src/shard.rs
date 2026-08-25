use crate::{
    sort_hits, SearchIndex, SearchQuery, SearchRecordColumns, SearchStreamBatch, SearchStreamStage,
};
use gfm_jobs::Cancellation;
use gfm_types::{FileId, FileRecord, GfmError, Result, SearchHit, VolumeId};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct ShardedSearchIndex {
    shards: BTreeMap<VolumeId, SearchIndex>,
}

impl ShardedSearchIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.shards.values().map(SearchIndex::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.shards.values().all(SearchIndex::is_empty)
    }

    pub fn shard_count(&self) -> usize {
        self.shards.len()
    }

    pub fn insert(&mut self, record: FileRecord) {
        self.remove_path(&record.path);
        self.shards
            .entry(record.id.volume)
            .or_default()
            .insert(record);
        self.prune_empty_shards();
    }

    pub fn apply_record_columns(&mut self, columns: SearchRecordColumns) -> bool {
        self.shards
            .get_mut(&columns.id.volume)
            .is_some_and(|shard| shard.apply_record_columns(columns))
    }

    pub fn remove(&mut self, id: FileId) -> Option<FileRecord> {
        let removed = self.shards.get_mut(&id.volume)?.remove(id);
        self.prune_empty_shards();
        removed
    }

    pub fn remove_path(&mut self, path: impl AsRef<Path>) -> Option<FileRecord> {
        let path = path.as_ref();
        let removed = self
            .shards
            .values_mut()
            .find_map(|shard| shard.remove_path(path));
        self.prune_empty_shards();
        removed
    }

    pub fn remove_subtree(&mut self, root: impl AsRef<Path>) -> Vec<FileRecord> {
        let root = root.as_ref();
        let mut removed = Vec::new();
        for shard in self.shards.values_mut() {
            removed.extend(shard.remove_subtree(root));
        }
        self.prune_empty_shards();
        removed
    }

    pub fn get_path(&self, path: impl AsRef<Path>) -> Option<&FileRecord> {
        let path = path.as_ref();
        self.shards.values().find_map(|shard| shard.get_path(path))
    }

    pub fn records(&self) -> impl Iterator<Item = &FileRecord> {
        self.shards.values().flat_map(SearchIndex::records)
    }

    pub fn insert_content(&mut self, id: FileId, text: &str) {
        if let Some(shard) = self.shards.get_mut(&id.volume) {
            shard.insert_content(id, text);
        }
    }

    pub fn import_content_postings(&mut self, postings: &[gfm_types::ContentPosting]) {
        for shard in self.shards.values_mut() {
            shard.import_content_postings(postings);
        }
    }

    pub fn content_postings(&self) -> Vec<gfm_types::ContentPosting> {
        self.shards
            .values()
            .flat_map(SearchIndex::content_postings)
            .collect()
    }

    pub fn query(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        self.query_structured(&SearchQuery::parse(query), limit)
    }

    pub fn query_structured(&self, query: &SearchQuery, limit: usize) -> Vec<SearchHit> {
        self.query_structured_cancellable(query, limit, &Cancellation::default())
            .unwrap_or_default()
    }

    pub fn query_cancellable(
        &self,
        query: &str,
        limit: usize,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchHit>> {
        self.query_structured_cancellable(&SearchQuery::parse(query), limit, cancellation)
    }

    pub fn query_structured_cancellable(
        &self,
        query: &SearchQuery,
        limit: usize,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchHit>> {
        cancellation.check()?;
        if query.is_empty() || limit == 0 || self.shards.is_empty() {
            return Ok(Vec::new());
        }

        let mut merged = Vec::new();
        std::thread::scope(|scope| {
            let handles: Vec<_> = self
                .shards
                .values()
                .map(|shard| {
                    scope.spawn(move || {
                        shard.query_structured_cancellable(query, limit, cancellation)
                    })
                })
                .collect();

            for handle in handles {
                let mut hits = handle
                    .join()
                    .map_err(|_| GfmError::Format("search shard worker panicked".to_string()))??;
                merged.append(&mut hits);
            }
            Ok::<(), GfmError>(())
        })?;

        sort_hits(&mut merged);
        merged.truncate(limit);
        cancellation.check()?;
        Ok(merged)
    }

    pub fn stream(&self, query: &str, limit: usize) -> Result<Vec<SearchStreamBatch>> {
        self.stream_structured(&SearchQuery::parse(query), limit)
    }

    pub fn stream_structured(
        &self,
        query: &SearchQuery,
        limit: usize,
    ) -> Result<Vec<SearchStreamBatch>> {
        self.stream_structured_cancellable(query, limit, &Cancellation::default())
    }

    pub fn stream_cancellable(
        &self,
        query: &str,
        limit: usize,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchStreamBatch>> {
        self.stream_structured_cancellable(&SearchQuery::parse(query), limit, cancellation)
    }

    pub fn stream_structured_cancellable(
        &self,
        query: &SearchQuery,
        limit: usize,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchStreamBatch>> {
        cancellation.check()?;
        if query.is_empty() || limit == 0 || self.shards.is_empty() {
            return Ok(Vec::new());
        }

        let mut hot = Vec::new();
        let mut deep = Vec::new();
        std::thread::scope(|scope| {
            let handles: Vec<_> = self
                .shards
                .values()
                .map(|shard| {
                    scope.spawn(move || {
                        shard.stream_structured_cancellable(query, limit, cancellation)
                    })
                })
                .collect();

            for handle in handles {
                for batch in handle
                    .join()
                    .map_err(|_| GfmError::Format("search shard worker panicked".to_string()))??
                {
                    match batch.stage {
                        SearchStreamStage::Hot => hot.extend(batch.hits),
                        SearchStreamStage::Deep => deep.extend(batch.hits),
                    }
                }
            }
            Ok::<(), GfmError>(())
        })?;

        let mut batches = Vec::new();
        let mut seen = BTreeMap::new();
        sort_hits(&mut hot);
        hot.truncate(limit);
        if !hot.is_empty() {
            for hit in &hot {
                seen.insert(hit.record.id, hit.score);
            }
            batches.push(SearchStreamBatch {
                stage: SearchStreamStage::Hot,
                hits: hot,
            });
        }

        sort_hits(&mut deep);
        deep.retain(|hit| match seen.get(&hit.record.id) {
            Some(score) => hit.score > *score,
            None => true,
        });
        deep.truncate(limit);
        if !deep.is_empty() {
            batches.push(SearchStreamBatch {
                stage: SearchStreamStage::Deep,
                hits: deep,
            });
        }
        cancellation.check()?;
        Ok(batches)
    }

    fn prune_empty_shards(&mut self) {
        self.shards.retain(|_, shard| !shard.is_empty());
    }
}
