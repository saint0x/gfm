use crate::{
    top_hits, BoundedHitMerge, SearchFuzzyPosting, SearchIndex, SearchLookup, SearchLookupBudget,
    SearchLookupIds, SearchLookupTelemetry, SearchLookupTerms, SearchMetadataPosting,
    SearchPrefixPosting, SearchQuery, SearchQueryReport, SearchRecordColumns, SearchStreamBatch,
    SearchStreamStage, SearchSubstringPosting,
};
use gfm_jobs::Cancellation;
use gfm_types::{
    ContentPositions, ContentPosting, FileId, FileRecord, GfmError, Result, SearchHit, VolumeId,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SearchVolumeScope {
    #[default]
    All,
    Only(BTreeSet<VolumeId>),
}

impl SearchVolumeScope {
    pub fn all() -> Self {
        Self::All
    }

    pub fn only(volumes: impl IntoIterator<Item = VolumeId>) -> Self {
        Self::Only(volumes.into_iter().collect())
    }

    pub fn allows(&self, volume: VolumeId) -> bool {
        match self {
            Self::All => true,
            Self::Only(volumes) => volumes.contains(&volume),
        }
    }
}

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

    pub fn insert_with_columns(
        &mut self,
        record: FileRecord,
        columns: SearchRecordColumns,
    ) -> bool {
        self.insert_with_columns_inner(record, columns, true)
    }

    pub fn insert_with_columns_deferred_fuzzy(
        &mut self,
        record: FileRecord,
        columns: SearchRecordColumns,
    ) -> bool {
        self.insert_with_columns_inner(record, columns, false)
    }

    pub fn insert_with_columns_deferred_sidecars(
        &mut self,
        record: FileRecord,
        columns: SearchRecordColumns,
    ) -> bool {
        self.remove_path(&record.path);
        let shard = self.shards.entry(record.id.volume).or_default();
        let inserted = shard.insert_with_columns_deferred_sidecars(record, columns);
        self.prune_empty_shards();
        inserted
    }

    fn insert_with_columns_inner(
        &mut self,
        record: FileRecord,
        columns: SearchRecordColumns,
        build_fuzzy: bool,
    ) -> bool {
        self.remove_path(&record.path);
        let shard = self.shards.entry(record.id.volume).or_default();
        let inserted = if build_fuzzy {
            shard.insert_with_columns(record, columns)
        } else {
            shard.insert_with_columns_deferred_fuzzy(record, columns)
        };
        self.prune_empty_shards();
        inserted
    }

    pub fn import_fuzzy_postings(&mut self, postings: &[SearchFuzzyPosting]) -> usize {
        self.shards
            .values_mut()
            .map(|shard| shard.import_fuzzy_postings(postings))
            .max()
            .unwrap_or(0)
    }

    pub fn import_prefix_postings(&mut self, postings: &[SearchPrefixPosting]) -> usize {
        let mut by_volume = partition_prefix_postings_by_volume(postings);
        for (volume, volume_postings) in &mut by_volume {
            if let Some(shard) = self.shards.get_mut(volume) {
                shard.import_prefix_postings(volume_postings);
            }
        }
        self.shards
            .values()
            .map(SearchIndex::indexed_name_prefixes)
            .sum()
    }

    pub fn import_substring_postings(&mut self, postings: &[SearchSubstringPosting]) -> usize {
        let mut imported = 0;
        let mut by_volume = partition_substring_postings_by_volume(postings);
        for (volume, volume_postings) in &mut by_volume {
            if let Some(shard) = self.shards.get_mut(volume) {
                imported += shard.import_substring_postings(volume_postings);
            }
        }
        imported
    }

    pub fn import_metadata_postings(&mut self, postings: &[SearchMetadataPosting]) -> usize {
        let mut imported = 0;
        let mut by_volume = partition_metadata_postings_by_volume(postings);
        for (volume, volume_postings) in &mut by_volume {
            if let Some(shard) = self.shards.get_mut(volume) {
                imported += shard.import_metadata_postings(volume_postings);
            }
        }
        imported
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

    pub fn import_content_postings(&mut self, postings: &[ContentPosting]) {
        let mut by_volume = partition_content_postings_by_volume(postings);
        for (volume, volume_postings) in &mut by_volume {
            if let Some(shard) = self.shards.get_mut(volume) {
                shard.import_content_postings(volume_postings);
            }
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
        self.query_structured_with_lookup_cancellable(
            query,
            limit,
            &crate::EmptySearchLookup,
            cancellation,
        )
    }

    pub fn query_structured_with_lookup_cancellable(
        &self,
        query: &SearchQuery,
        limit: usize,
        lookup: &dyn SearchLookup,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchHit>> {
        Ok(self
            .query_structured_with_lookup_budget_cancellable(
                query,
                limit,
                lookup,
                SearchLookupBudget::default(),
                cancellation,
            )?
            .hits)
    }

    pub fn query_structured_with_lookup_budget_cancellable(
        &self,
        query: &SearchQuery,
        limit: usize,
        lookup: &dyn SearchLookup,
        budget: SearchLookupBudget,
        cancellation: &Cancellation,
    ) -> Result<SearchQueryReport> {
        self.query_structured_with_volume_scope_lookup_budget_cancellable(
            query,
            limit,
            &SearchVolumeScope::All,
            lookup,
            budget,
            cancellation,
        )
    }

    pub fn query_structured_with_volume_scope_cancellable(
        &self,
        query: &SearchQuery,
        limit: usize,
        scope: &SearchVolumeScope,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchHit>> {
        Ok(self
            .query_structured_with_volume_scope_lookup_budget_cancellable(
                query,
                limit,
                scope,
                &crate::EmptySearchLookup,
                SearchLookupBudget::default(),
                cancellation,
            )?
            .hits)
    }

    pub fn query_structured_with_volume_scope_lookup_budget_cancellable(
        &self,
        query: &SearchQuery,
        limit: usize,
        scope: &SearchVolumeScope,
        lookup: &dyn SearchLookup,
        budget: SearchLookupBudget,
        cancellation: &Cancellation,
    ) -> Result<SearchQueryReport> {
        cancellation.check()?;
        if query.is_empty() || limit == 0 || self.shards.is_empty() {
            return Ok(SearchQueryReport {
                hits: Vec::new(),
                lookup: SearchLookupTelemetry::default(),
            });
        }
        let shards = self.scoped_shards(scope);
        if shards.is_empty() {
            return Ok(SearchQueryReport {
                hits: Vec::new(),
                lookup: SearchLookupTelemetry::default(),
            });
        }
        if let [(volume, shard)] = shards.as_slice() {
            let scoped_lookup = VolumeScopedSearchLookup {
                lookup,
                volume: *volume,
            };
            return shard.query_structured_with_lookup_budget_cancellable(
                query,
                limit,
                &scoped_lookup,
                budget,
                cancellation,
            );
        }

        let mut merged = BoundedHitMerge::new(limit);
        let mut telemetry = SearchLookupTelemetry::default();
        std::thread::scope(|thread_scope| {
            let handles: Vec<_> = self
                .scoped_shards(scope)
                .into_iter()
                .map(|(volume, shard)| {
                    thread_scope.spawn(move || {
                        let scoped_lookup = VolumeScopedSearchLookup { lookup, volume };
                        shard.query_structured_with_lookup_budget_cancellable(
                            query,
                            limit,
                            &scoped_lookup,
                            budget,
                            cancellation,
                        )
                    })
                })
                .collect();

            for handle in handles {
                let report = handle
                    .join()
                    .map_err(|_| GfmError::Format("search shard worker panicked".to_string()))??;
                telemetry.merge(&report.lookup);
                merged.extend(report.hits);
            }
            Ok::<(), GfmError>(())
        })?;

        let merged = merged.into_sorted_hits();
        cancellation.check()?;
        Ok(SearchQueryReport {
            hits: merged,
            lookup: telemetry,
        })
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
        self.stream_structured_with_volume_scope_cancellable(
            query,
            limit,
            &SearchVolumeScope::All,
            cancellation,
        )
    }

    pub fn stream_structured_with_volume_scope_cancellable(
        &self,
        query: &SearchQuery,
        limit: usize,
        scope: &SearchVolumeScope,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchStreamBatch>> {
        cancellation.check()?;
        if query.is_empty() || limit == 0 || self.shards.is_empty() {
            return Ok(Vec::new());
        }
        let shards = self.scoped_shards(scope);
        if shards.is_empty() {
            return Ok(Vec::new());
        }
        if let [(_, shard)] = shards.as_slice() {
            return shard.stream_structured_cancellable(query, limit, cancellation);
        }

        let mut hot = BoundedHitMerge::new(limit);
        let mut deep = BoundedHitMerge::new(limit.saturating_mul(2));
        std::thread::scope(|thread_scope| {
            let handles: Vec<_> = self
                .scoped_shards(scope)
                .into_iter()
                .map(|(_, shard)| {
                    thread_scope.spawn(move || {
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
        let hot = hot.into_sorted_hits();
        if !hot.is_empty() {
            for hit in &hot {
                seen.insert(hit.record.id, hit.score);
            }
            batches.push(SearchStreamBatch {
                stage: SearchStreamStage::Hot,
                hits: hot,
            });
        }

        let mut deep = deep.into_sorted_hits();
        deep.retain(|hit| match seen.get(&hit.record.id) {
            Some(score) => hit.score > *score,
            None => true,
        });
        let deep = top_hits(deep, limit);
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

    fn scoped_shards(&self, scope: &SearchVolumeScope) -> Vec<(VolumeId, &SearchIndex)> {
        self.shards
            .iter()
            .filter_map(|(volume, shard)| scope.allows(*volume).then_some((*volume, shard)))
            .collect()
    }
}

struct VolumeScopedSearchLookup<'a> {
    lookup: &'a dyn SearchLookup,
    volume: VolumeId,
}

impl SearchLookup for VolumeScopedSearchLookup<'_> {
    fn prefix_ids(&self, prefix: &str) -> Result<Vec<FileId>> {
        self.lookup.prefix_ids_for_volume(prefix, self.volume)
    }

    fn substring_ids(&self, gram: &str) -> Result<Vec<FileId>> {
        self.lookup.substring_ids_for_volume(gram, self.volume)
    }

    fn prefix_ids_bounded(&self, prefix: &str, limit: usize) -> Result<SearchLookupIds> {
        self.lookup
            .prefix_ids_for_volume_bounded(prefix, self.volume, limit)
    }

    fn substring_ids_bounded(&self, gram: &str, limit: usize) -> Result<SearchLookupIds> {
        self.lookup
            .substring_ids_for_volume_bounded(gram, self.volume, limit)
    }

    fn fuzzy_terms(&self, key: &str) -> Result<Vec<String>> {
        self.lookup.fuzzy_terms(key)
    }

    fn fuzzy_terms_bounded(&self, key: &str, limit: usize) -> Result<SearchLookupTerms> {
        self.lookup.fuzzy_terms_bounded(key, limit)
    }

    fn cache_telemetry(&self) -> SearchLookupTelemetry {
        self.lookup.cache_telemetry()
    }
}

fn partition_prefix_postings_by_volume(
    postings: &[SearchPrefixPosting],
) -> BTreeMap<VolumeId, Vec<SearchPrefixPosting>> {
    let mut by_volume: BTreeMap<VolumeId, Vec<SearchPrefixPosting>> = BTreeMap::new();
    for posting in postings {
        let mut ids_by_volume: BTreeMap<VolumeId, Vec<FileId>> = BTreeMap::new();
        for id in &posting.ids {
            ids_by_volume.entry(id.volume).or_default().push(*id);
        }
        for (volume, ids) in ids_by_volume {
            by_volume
                .entry(volume)
                .or_default()
                .push(SearchPrefixPosting {
                    prefix: posting.prefix.clone(),
                    ids,
                });
        }
    }
    by_volume
}

fn partition_substring_postings_by_volume(
    postings: &[SearchSubstringPosting],
) -> BTreeMap<VolumeId, Vec<SearchSubstringPosting>> {
    let mut by_volume: BTreeMap<VolumeId, Vec<SearchSubstringPosting>> = BTreeMap::new();
    for posting in postings {
        let mut ids_by_volume: BTreeMap<VolumeId, Vec<FileId>> = BTreeMap::new();
        for id in &posting.ids {
            ids_by_volume.entry(id.volume).or_default().push(*id);
        }
        for (volume, ids) in ids_by_volume {
            by_volume
                .entry(volume)
                .or_default()
                .push(SearchSubstringPosting {
                    gram: posting.gram.clone(),
                    ids,
                });
        }
    }
    by_volume
}

fn partition_metadata_postings_by_volume(
    postings: &[SearchMetadataPosting],
) -> BTreeMap<VolumeId, Vec<SearchMetadataPosting>> {
    let mut by_volume: BTreeMap<VolumeId, Vec<SearchMetadataPosting>> = BTreeMap::new();
    for posting in postings {
        let mut ids_by_volume: BTreeMap<VolumeId, Vec<FileId>> = BTreeMap::new();
        for id in &posting.ids {
            ids_by_volume.entry(id.volume).or_default().push(*id);
        }
        for (volume, ids) in ids_by_volume {
            by_volume
                .entry(volume)
                .or_default()
                .push(SearchMetadataPosting {
                    field: posting.field,
                    term: posting.term.clone(),
                    ids,
                });
        }
    }
    by_volume
}

fn partition_content_postings_by_volume(
    postings: &[ContentPosting],
) -> BTreeMap<VolumeId, Vec<ContentPosting>> {
    let mut by_volume: BTreeMap<VolumeId, Vec<ContentPosting>> = BTreeMap::new();
    for posting in postings {
        let mut ids_by_volume: BTreeMap<VolumeId, Vec<FileId>> = BTreeMap::new();
        let mut positions_by_volume: BTreeMap<VolumeId, Vec<ContentPositions>> = BTreeMap::new();
        for id in &posting.ids {
            ids_by_volume.entry(id.volume).or_default().push(*id);
        }
        for positions in &posting.positions {
            positions_by_volume
                .entry(positions.id.volume)
                .or_default()
                .push(positions.clone());
        }
        let volumes = ids_by_volume
            .keys()
            .chain(positions_by_volume.keys())
            .copied()
            .collect::<BTreeSet<_>>();
        for volume in volumes {
            by_volume.entry(volume).or_default().push(ContentPosting {
                term: posting.term.clone(),
                ids: ids_by_volume.remove(&volume).unwrap_or_default(),
                positions: positions_by_volume.remove(&volume).unwrap_or_default(),
            });
        }
    }
    by_volume
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SearchMetadataField;

    #[test]
    fn partitions_id_sidecar_postings_by_volume_in_one_pass_shape() {
        let first = FileId::new(VolumeId(1), 11);
        let second = FileId::new(VolumeId(2), 22);
        let missing = FileId::new(VolumeId(9), 99);

        let prefixes = partition_prefix_postings_by_volume(&[SearchPrefixPosting {
            prefix: "proj".to_string(),
            ids: vec![first, second, missing],
        }]);
        assert_eq!(
            prefixes.keys().copied().collect::<Vec<_>>(),
            vec![VolumeId(1), VolumeId(2), VolumeId(9),]
        );
        assert_eq!(prefixes[&VolumeId(1)][0].ids, vec![first]);
        assert_eq!(prefixes[&VolumeId(2)][0].ids, vec![second]);

        let substrings = partition_substring_postings_by_volume(&[SearchSubstringPosting {
            gram: "roj".to_string(),
            ids: vec![second, first],
        }]);
        assert_eq!(substrings[&VolumeId(1)][0].ids, vec![first]);
        assert_eq!(substrings[&VolumeId(2)][0].ids, vec![second]);

        let metadata = partition_metadata_postings_by_volume(&[SearchMetadataPosting {
            field: SearchMetadataField::Tag,
            term: "important".to_string(),
            ids: vec![missing, first],
        }]);
        assert_eq!(metadata[&VolumeId(1)][0].ids, vec![first]);
        assert_eq!(metadata[&VolumeId(9)][0].ids, vec![missing]);
    }

    #[test]
    fn partitions_content_postings_by_ids_and_positions_volume() {
        let first = FileId::new(VolumeId(1), 11);
        let second = FileId::new(VolumeId(2), 22);
        let positions_only = FileId::new(VolumeId(3), 33);

        let content = partition_content_postings_by_volume(&[ContentPosting {
            term: "bodymarker".to_string(),
            ids: vec![first, second],
            positions: vec![
                ContentPositions {
                    id: first,
                    positions: vec![1, 3],
                },
                ContentPositions {
                    id: positions_only,
                    positions: vec![5],
                },
            ],
        }]);

        assert_eq!(
            content.keys().copied().collect::<Vec<_>>(),
            vec![VolumeId(1), VolumeId(2), VolumeId(3),]
        );
        assert_eq!(content[&VolumeId(1)][0].ids, vec![first]);
        assert_eq!(content[&VolumeId(1)][0].positions[0].positions, vec![1, 3]);
        assert_eq!(content[&VolumeId(2)][0].ids, vec![second]);
        assert!(content[&VolumeId(2)][0].positions.is_empty());
        assert!(content[&VolumeId(3)][0].ids.is_empty());
        assert_eq!(content[&VolumeId(3)][0].positions[0].id, positions_only);
    }
}
