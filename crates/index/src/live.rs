use crate::lookup::sidecar_candidate_ids_cancellable;
use crate::{
    content_query_terms, ContentIndexBatchReport, ContentQueryLoadReport, MetadataUpdateReport,
    RenameCorrelationReport, SidecarQueryImport, SidecarRecordHydrationReport,
};
use gfm_content::{
    ExtractionFingerprint, ExtractionQuarantine, ExtractionStatus, Extractor, QuarantineDecision,
};
use gfm_jobs::Cancellation;
use gfm_search::{
    SearchFuzzyPosting, SearchLookup, SearchLookupBudget, SearchMetadataPosting,
    SearchPrefixPosting, SearchQuery, SearchQueryReport, SearchRecordColumns, SearchStreamBatch,
    SearchSubstringPosting, SearchVolumeScope, ShardedSearchIndex,
};
use gfm_store::{
    read_content_postings, write_content_postings, MmapContentArchive, MmapContentSet,
    MmapRecordArchive, MmapRecordColumns,
};
use gfm_types::{
    ContentPosting, FileEvent, FileEventKind, FileId, FileKind, FileRecord, Result, SearchHit,
};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct LiveIndex {
    index: ShardedSearchIndex,
}

impl LiveIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn indexed_records(&self) -> usize {
        self.index.len()
    }

    pub fn from_records(records: Vec<FileRecord>) -> Self {
        let mut live = Self::new();
        for record in records {
            live.index.insert(record);
        }
        live
    }

    pub fn from_records_with_columns(
        records: Vec<FileRecord>,
        columns: Vec<SearchRecordColumns>,
    ) -> (Self, usize) {
        let mut live = Self::new();
        let mut columns_by_id = columns
            .into_iter()
            .map(|columns| (columns.id, columns))
            .collect::<HashMap<_, _>>();
        let mut applied = 0usize;
        for record in records {
            if let Some(columns) = columns_by_id.remove(&record.id) {
                if live.index.insert_with_columns(record, columns) {
                    applied += 1;
                }
            } else {
                live.index.insert(record);
            }
        }
        (live, applied)
    }

    pub fn from_records_with_columns_and_fuzzy(
        records: Vec<FileRecord>,
        columns: Vec<SearchRecordColumns>,
        fuzzy: Vec<SearchFuzzyPosting>,
    ) -> (Self, usize, usize) {
        let mut live = Self::new();
        let mut columns_by_id = columns
            .into_iter()
            .map(|columns| (columns.id, columns))
            .collect::<HashMap<_, _>>();
        let mut applied = 0usize;
        for record in records {
            if let Some(columns) = columns_by_id.remove(&record.id) {
                if live
                    .index
                    .insert_with_columns_deferred_fuzzy(record, columns)
                {
                    applied += 1;
                }
            } else {
                live.index.insert(record);
            }
        }
        let fuzzy_keys = live.index.import_fuzzy_postings(&fuzzy);
        (live, applied, fuzzy_keys)
    }

    pub fn from_records_deferred_sidecars(records: Vec<FileRecord>) -> Self {
        let mut live = Self::new();
        for record in records {
            let columns = SearchRecordColumns {
                id: record.id,
                name: record.name.clone(),
                path: record.path.to_string_lossy().into_owned(),
                extension: record.extension().map(ToOwned::to_owned),
                tags: record.tags.clone(),
                comment: record.finder_comment.clone(),
            };
            live.index
                .insert_with_columns_deferred_sidecars(record, columns);
        }
        live
    }

    pub fn from_records_with_sidecars(
        records: Vec<FileRecord>,
        columns: Vec<SearchRecordColumns>,
        metadata: Vec<SearchMetadataPosting>,
        prefixes: Vec<SearchPrefixPosting>,
        substrings: Vec<SearchSubstringPosting>,
        fuzzy: Vec<SearchFuzzyPosting>,
        content: Vec<ContentPosting>,
    ) -> (Self, usize, usize, usize, usize, usize, usize) {
        let mut live = Self::new();
        let mut columns_by_id = columns
            .into_iter()
            .map(|columns| (columns.id, columns))
            .collect::<HashMap<_, _>>();
        let mut applied = 0usize;
        for record in records {
            if let Some(columns) = columns_by_id.remove(&record.id) {
                if live
                    .index
                    .insert_with_columns_deferred_sidecars(record, columns)
                {
                    applied += 1;
                }
            } else {
                live.index.insert(record);
            }
        }
        let metadata_keys = live.index.import_metadata_postings(&metadata);
        let prefix_keys = live.index.import_prefix_postings(&prefixes);
        let substring_keys = live.index.import_substring_postings(&substrings);
        let fuzzy_keys = live.index.import_fuzzy_postings(&fuzzy);
        let content_keys = content.len();
        live.index.import_content_postings(&content);
        (
            live,
            applied,
            metadata_keys,
            prefix_keys,
            substring_keys,
            fuzzy_keys,
            content_keys,
        )
    }

    pub fn from_mmap_records_with_sidecar_import(
        records: &MmapRecordArchive,
        columns: &MmapRecordColumns,
        import: SidecarQueryImport,
    ) -> Result<(Self, SidecarRecordHydrationReport)> {
        let mut live = Self::new();
        let mut loaded = 0usize;
        let mut missing = 0usize;
        let mut applied = 0usize;

        if import.report.requires_full_record_hydration {
            for index in 0..records.len() {
                let record = records.record(index)?;
                if insert_mmap_record_with_columns(&mut live, columns, record)? {
                    applied += 1;
                }
                loaded += 1;
            }
        } else {
            let cancellation = Cancellation::default();
            let candidate_ids = sidecar_candidate_ids_cancellable(&import, &cancellation)?;
            let batch = records.records_for_sorted_ids(candidate_ids)?;
            missing = batch.missing;
            loaded = batch.records.len();
            for record in batch.records {
                if insert_mmap_record_with_columns(&mut live, columns, record)? {
                    applied += 1;
                }
            }
        }

        let metadata_keys = live.index.import_metadata_postings(&import.metadata);
        let prefix_keys = live.index.import_prefix_postings(&import.prefixes);
        let substring_keys = live.index.import_substring_postings(&import.substrings);
        let fuzzy_keys = live.index.import_fuzzy_postings(&import.fuzzy);
        let content_keys = import.content.len();
        live.index.import_content_postings(&import.content);
        let report = SidecarRecordHydrationReport {
            records_loaded: loaded,
            records_missing: missing,
            columns_applied: applied,
            metadata_keys,
            prefix_keys,
            substring_keys,
            fuzzy_keys,
            content_keys,
            import: import.report,
        };
        Ok((live, report))
    }

    pub fn from_mmap_records_with_content_postings(
        records: &MmapRecordArchive,
        postings: Vec<ContentPosting>,
    ) -> Result<(Self, ContentQueryLoadReport)> {
        Self::from_mmap_records_with_content_postings_cancellable(
            records,
            postings,
            &Cancellation::default(),
        )
    }

    pub fn from_mmap_records_with_content_postings_cancellable(
        records: &MmapRecordArchive,
        postings: Vec<ContentPosting>,
        cancellation: &Cancellation,
    ) -> Result<(Self, ContentQueryLoadReport)> {
        let candidate_ids = content_candidate_ids_cancellable(&postings, cancellation)?;
        let full_hydration = candidate_ids.is_empty();
        let mut live = Self::new();
        let mut loaded = 0usize;
        let mut missing = 0usize;

        if full_hydration {
            for index in 0..records.len() {
                cancellation.check()?;
                live.index.insert(records.record(index)?);
                loaded += 1;
            }
        } else {
            let batch = records.records_for_sorted_ids(candidate_ids.iter().copied())?;
            missing = batch.missing;
            loaded = batch.records.len();
            for record in batch.records {
                cancellation.check()?;
                live.index.insert(record);
            }
        }

        let content_keys = postings.len();
        live.index.import_content_postings(&postings);
        Ok((
            live,
            ContentQueryLoadReport {
                content_keys,
                candidate_ids: candidate_ids.len(),
                records_loaded: loaded,
                records_missing: missing,
                full_hydration,
            },
        ))
    }

    pub fn apply_record_columns(&mut self, columns: SearchRecordColumns) -> bool {
        self.index.apply_record_columns(columns)
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        self.index.query(query, limit)
    }

    pub fn search_with_lookup(
        &self,
        query: &str,
        limit: usize,
        lookup: &dyn SearchLookup,
    ) -> Result<Vec<SearchHit>> {
        self.search_structured_with_lookup(&SearchQuery::parse(query), limit, lookup)
    }

    pub fn search_structured_with_lookup(
        &self,
        query: &SearchQuery,
        limit: usize,
        lookup: &dyn SearchLookup,
    ) -> Result<Vec<SearchHit>> {
        self.index.query_structured_with_lookup_cancellable(
            query,
            limit,
            lookup,
            &Cancellation::default(),
        )
    }

    pub fn search_with_lookup_budget(
        &self,
        query: &str,
        limit: usize,
        lookup: &dyn SearchLookup,
        budget: SearchLookupBudget,
    ) -> Result<SearchQueryReport> {
        self.search_structured_with_lookup_budget(&SearchQuery::parse(query), limit, lookup, budget)
    }

    pub fn search_structured_with_lookup_budget(
        &self,
        query: &SearchQuery,
        limit: usize,
        lookup: &dyn SearchLookup,
        budget: SearchLookupBudget,
    ) -> Result<SearchQueryReport> {
        self.search_structured_with_lookup_budget_cancellable(
            query,
            limit,
            lookup,
            budget,
            &Cancellation::default(),
        )
    }

    pub fn search_with_lookup_budget_cancellable(
        &self,
        query: &str,
        limit: usize,
        lookup: &dyn SearchLookup,
        budget: SearchLookupBudget,
        cancellation: &Cancellation,
    ) -> Result<SearchQueryReport> {
        self.search_structured_with_lookup_budget_cancellable(
            &SearchQuery::parse(query),
            limit,
            lookup,
            budget,
            cancellation,
        )
    }

    pub fn search_structured_with_lookup_budget_cancellable(
        &self,
        query: &SearchQuery,
        limit: usize,
        lookup: &dyn SearchLookup,
        budget: SearchLookupBudget,
        cancellation: &Cancellation,
    ) -> Result<SearchQueryReport> {
        let cache_before = lookup.cache_telemetry();
        let mut report = self.index.query_structured_with_lookup_budget_cancellable(
            query,
            limit,
            lookup,
            budget,
            cancellation,
        )?;
        let cache_after = lookup.cache_telemetry();
        report.lookup.merge_cache_delta(&cache_before, &cache_after);
        Ok(report)
    }

    pub fn search_with_volume_scope(
        &self,
        query: &str,
        limit: usize,
        scope: &SearchVolumeScope,
    ) -> Result<Vec<SearchHit>> {
        self.search_with_volume_scope_cancellable(query, limit, scope, &Cancellation::default())
    }

    pub fn search_with_volume_scope_cancellable(
        &self,
        query: &str,
        limit: usize,
        scope: &SearchVolumeScope,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchHit>> {
        self.search_structured_with_volume_scope_cancellable(
            &SearchQuery::parse(query),
            limit,
            scope,
            cancellation,
        )
    }

    pub fn search_structured_with_volume_scope_cancellable(
        &self,
        query: &SearchQuery,
        limit: usize,
        scope: &SearchVolumeScope,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchHit>> {
        self.index
            .query_structured_with_volume_scope_cancellable(query, limit, scope, cancellation)
    }

    pub fn search_with_volume_scope_lookup_budget_cancellable(
        &self,
        query: &str,
        limit: usize,
        scope: &SearchVolumeScope,
        lookup: &dyn SearchLookup,
        budget: SearchLookupBudget,
        cancellation: &Cancellation,
    ) -> Result<SearchQueryReport> {
        self.search_structured_with_volume_scope_lookup_budget_cancellable(
            &SearchQuery::parse(query),
            limit,
            scope,
            lookup,
            budget,
            cancellation,
        )
    }

    pub fn search_structured_with_volume_scope_lookup_budget_cancellable(
        &self,
        query: &SearchQuery,
        limit: usize,
        scope: &SearchVolumeScope,
        lookup: &dyn SearchLookup,
        budget: SearchLookupBudget,
        cancellation: &Cancellation,
    ) -> Result<SearchQueryReport> {
        let cache_before = lookup.cache_telemetry();
        let mut report = self
            .index
            .query_structured_with_volume_scope_lookup_budget_cancellable(
                query,
                limit,
                scope,
                lookup,
                budget,
                cancellation,
            )?;
        let cache_after = lookup.cache_telemetry();
        report.lookup.merge_cache_delta(&cache_before, &cache_after);
        Ok(report)
    }

    pub fn stream_search(&self, query: &str, limit: usize) -> Result<Vec<SearchStreamBatch>> {
        self.stream_structured_search(&SearchQuery::parse(query), limit)
    }

    pub fn stream_structured_search(
        &self,
        query: &SearchQuery,
        limit: usize,
    ) -> Result<Vec<SearchStreamBatch>> {
        self.index.stream_structured(query, limit)
    }

    pub fn stream_search_with_volume_scope(
        &self,
        query: &str,
        limit: usize,
        scope: &SearchVolumeScope,
    ) -> Result<Vec<SearchStreamBatch>> {
        self.index.stream_structured_with_volume_scope_cancellable(
            &SearchQuery::parse(query),
            limit,
            scope,
            &Cancellation::default(),
        )
    }

    pub fn search_cancellable(
        &self,
        query: &str,
        limit: usize,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchHit>> {
        self.index.query_cancellable(query, limit, cancellation)
    }

    pub fn search_with_snippets(
        &self,
        query: &str,
        limit: usize,
        extractor: &Extractor,
        context_bytes: usize,
    ) -> Result<Vec<SearchHit>> {
        self.search_with_snippets_cancellable(
            query,
            limit,
            extractor,
            context_bytes,
            &Cancellation::default(),
        )
    }

    pub fn search_with_snippets_cancellable(
        &self,
        query: &str,
        limit: usize,
        extractor: &Extractor,
        context_bytes: usize,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchHit>> {
        cancellation.check()?;
        let parsed = SearchQuery::parse(query);
        cancellation.check()?;
        let mut hits = self
            .index
            .query_structured_cancellable(&parsed, limit, cancellation)?;
        for hit in &mut hits {
            cancellation.check()?;
            if matches!(hit.reason, gfm_types::MatchReason::Content) {
                hit.snippet = extractor.snippet_for_record(
                    &hit.record,
                    &parsed.terms,
                    &parsed.phrases,
                    context_bytes,
                )?;
                cancellation.check()?;
            }
        }
        Ok(hits)
    }

    pub fn index_content(&mut self, extractor: &Extractor) -> Result<usize> {
        self.index_content_cancellable(extractor, &Cancellation::default())
    }

    pub fn index_content_cancellable(
        &mut self,
        extractor: &Extractor,
        cancellation: &Cancellation,
    ) -> Result<usize> {
        let records: Vec<_> = self.index.records().cloned().collect();
        let mut indexed = 0;
        for record in records {
            cancellation.check()?;
            if let Some(document) = extractor.extract_record(&record)? {
                cancellation.check()?;
                self.index.insert_content(record.id, &document.text);
                indexed += 1;
            }
        }
        Ok(indexed)
    }

    pub fn index_content_with_quarantine(
        &mut self,
        extractor: &Extractor,
        quarantine: &mut ExtractionQuarantine,
    ) -> Result<ContentIndexBatchReport> {
        self.index_content_with_quarantine_cancellable(
            extractor,
            quarantine,
            &Cancellation::default(),
        )
    }

    pub fn index_content_with_quarantine_cancellable(
        &mut self,
        extractor: &Extractor,
        quarantine: &mut ExtractionQuarantine,
        cancellation: &Cancellation,
    ) -> Result<ContentIndexBatchReport> {
        let records: Vec<_> = self.index.records().cloned().collect();
        let mut report = ContentIndexBatchReport::default();
        for record in records {
            cancellation.check()?;
            if record.kind != FileKind::File {
                report.skipped += 1;
                continue;
            }

            let fingerprint = ExtractionFingerprint::for_path(&record.path)?;
            cancellation.check()?;
            if matches!(
                quarantine.before_extract(&record.path, &fingerprint),
                QuarantineDecision::Quarantined(_)
            ) {
                report.skipped += 1;
                report.quarantined += 1;
                continue;
            }

            let extraction = extractor.extract_path_report(&record.path)?;
            cancellation.check()?;
            let status = extraction.status.clone();
            let decision = quarantine.record_report(&extraction);
            if let Some(document) = extraction.document {
                self.index.insert_content(record.id, &document.text);
                report.indexed += 1;
            } else {
                report.skipped += 1;
            }
            if matches!(status, ExtractionStatus::Quarantined(_))
                || matches!(decision, QuarantineDecision::Quarantined(_))
            {
                report.quarantined += 1;
            }
        }
        Ok(report)
    }

    pub fn save_content_postings(&self, path: impl AsRef<Path>) -> Result<()> {
        write_content_postings(path, &self.index.content_postings())
    }

    pub fn content_postings(&self) -> Vec<gfm_types::ContentPosting> {
        self.index.content_postings()
    }

    pub fn load_content_postings(&mut self, path: impl AsRef<Path>) -> Result<usize> {
        let postings = read_content_postings(path)?;
        let terms = postings.len();
        self.index.import_content_postings(&postings);
        Ok(terms)
    }

    pub fn load_content_postings_with_budget(
        &mut self,
        path: impl AsRef<Path>,
        query: &str,
        budget: SearchLookupBudget,
    ) -> Result<usize> {
        let content = MmapContentArchive::open(path)?;
        let postings = content.postings_for_terms_limit(
            content_query_terms(query),
            budget.max_content_ids_per_term,
        )?;
        let terms = postings.len();
        self.index.import_content_postings(&postings);
        Ok(terms)
    }

    pub fn load_content_set_postings(
        &mut self,
        paths: &[impl AsRef<Path>],
        query: &str,
    ) -> Result<usize> {
        let content = MmapContentSet::open(paths)?;
        let postings = content.postings_for_terms(content_query_terms(query))?;
        let terms = postings.len();
        self.index.import_content_postings(&postings);
        Ok(terms)
    }

    pub fn load_content_set_postings_with_budget(
        &mut self,
        paths: &[impl AsRef<Path>],
        query: &str,
        budget: SearchLookupBudget,
    ) -> Result<usize> {
        let content = MmapContentSet::open(paths)?;
        let postings = content.postings_for_terms_limit(
            content_query_terms(query),
            budget.max_content_ids_per_term,
        )?;
        let terms = postings.len();
        self.index.import_content_postings(&postings);
        Ok(terms)
    }

    pub fn load_content_manifest_postings(
        &mut self,
        manifest_path: impl AsRef<Path>,
        query: &str,
    ) -> Result<usize> {
        let content = MmapContentSet::open_manifest(manifest_path)?;
        let postings = content.postings_for_terms(content_query_terms(query))?;
        let terms = postings.len();
        self.index.import_content_postings(&postings);
        Ok(terms)
    }

    pub fn load_content_manifest_postings_with_budget(
        &mut self,
        manifest_path: impl AsRef<Path>,
        query: &str,
        budget: SearchLookupBudget,
    ) -> Result<usize> {
        let content = MmapContentSet::open_manifest(manifest_path)?;
        let postings = content.postings_for_terms_limit(
            content_query_terms(query),
            budget.max_content_ids_per_term,
        )?;
        let terms = postings.len();
        self.index.import_content_postings(&postings);
        Ok(terms)
    }

    pub fn apply_event(&mut self, event: &FileEvent) -> Result<UpdateOutcome> {
        match &event.kind {
            FileEventKind::Create | FileEventKind::Other => self.upsert_path(&event.path),
            FileEventKind::Metadata | FileEventKind::Modify => {
                let report = self.apply_metadata_update(&event.path)?;
                Ok(UpdateOutcome::MetadataUpdated {
                    changed: report.changed.len(),
                })
            }
            FileEventKind::Remove => {
                let removed = self.index.remove_subtree(&event.path).len();
                Ok(UpdateOutcome::Removed { records: removed })
            }
            FileEventKind::Rename { from, to } => {
                let report = self.apply_rename(from, to)?;
                Ok(UpdateOutcome::Renamed {
                    removed: report.removed,
                    inserted: report.inserted,
                })
            }
            FileEventKind::Rescan => Ok(UpdateOutcome::NeedsRescan),
        }
    }

    pub fn apply_rename(&mut self, from: &Path, to: &Path) -> Result<RenameCorrelationReport> {
        crate::correlate_rename(&mut self.index, from, to)
    }

    pub fn apply_metadata_update(&mut self, path: &Path) -> Result<MetadataUpdateReport> {
        let previous = self.index.get_path(path).cloned();
        let current =
            gfm_fs::record_for_path(path, previous.as_ref().and_then(|r| r.parent), false)?;
        let report = MetadataUpdateReport::from_records(path, previous.as_ref(), &current);
        self.index.insert(current);
        Ok(report)
    }

    fn upsert_path(&mut self, path: &Path) -> Result<UpdateOutcome> {
        let record = gfm_fs::record_for_path(path, None, false)?;
        self.index.insert(record);
        Ok(UpdateOutcome::Upserted)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateOutcome {
    Upserted,
    MetadataUpdated { changed: usize },
    Removed { records: usize },
    Renamed { removed: usize, inserted: usize },
    NeedsRescan,
}

fn insert_mmap_record_with_columns(
    live: &mut LiveIndex,
    columns: &MmapRecordColumns,
    record: FileRecord,
) -> Result<bool> {
    if let Some(column) = columns.find(record.id)? {
        Ok(live.index.insert_with_columns_deferred_sidecars(
            record,
            SearchRecordColumns {
                id: column.id,
                name: column.name,
                path: column.path,
                extension: column.extension,
                tags: column.tags,
                comment: column.comment,
            },
        ))
    } else {
        live.index.insert(record);
        Ok(false)
    }
}

fn content_candidate_ids_cancellable(
    postings: &[ContentPosting],
    cancellation: &Cancellation,
) -> Result<BTreeSet<FileId>> {
    let mut ids = BTreeSet::new();
    for posting in postings {
        cancellation.check()?;
        for id in &posting.ids {
            cancellation.check()?;
            ids.insert(*id);
        }
        for positions in &posting.positions {
            cancellation.check()?;
            ids.insert(positions.id);
        }
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_types::{ContentPositions, GfmError, VolumeId};

    #[test]
    fn mmap_content_candidate_expansion_honors_cancelled_tokens() {
        let cancellation = Cancellation::default();
        cancellation.cancel();
        let posting = ContentPosting {
            term: "needle".to_string(),
            ids: vec![FileId::new(VolumeId(1), 1)],
            positions: vec![ContentPositions {
                id: FileId::new(VolumeId(1), 2),
                positions: vec![0],
            }],
        };

        let result = content_candidate_ids_cancellable(&[posting], &cancellation);

        assert!(matches!(result, Err(GfmError::Cancelled)));
    }
}
