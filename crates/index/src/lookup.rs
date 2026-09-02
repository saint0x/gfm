use gfm_jobs::Cancellation;
use gfm_search::substring_candidate_grams;
use gfm_search::{
    SearchFuzzyPosting, SearchLookup, SearchLookupBudget, SearchLookupIds, SearchLookupTelemetry,
    SearchLookupTerms, SearchMetadataField, SearchMetadataPosting, SearchPrefixPosting,
    SearchQuery, SearchQueryReport, SearchRecordColumns, SearchSubstringPosting, SearchVolumeScope,
};
use gfm_store::{
    MetadataField, MetadataPosting, MmapContentArchive, MmapFuzzyArchive, MmapMetadataArchive,
    MmapPrefixArchive, MmapRecordArchive, MmapRecordColumns, MmapSubstringArchive,
};
use gfm_types::{ContentPosting, FileId, FileRecord, Result, VolumeId};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::Path;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex, MutexGuard,
};

const SEARCH_ARCHIVE_LOOKUP_CACHE_CAPACITY: usize = 512;
const SIDECAR_RECORD_CACHE_CAPACITY: usize = 8192;
const SIDECAR_CONTENT_POSTING_CACHE_CAPACITY: usize = 512;
const SIDECAR_QUERY_RESULT_CACHE_CAPACITY: usize = 256;
const SIDECAR_CONTENT_TERM_CHECK_STRIDE: usize = 256;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SidecarQueryImport {
    pub metadata: Vec<SearchMetadataPosting>,
    pub prefixes: Vec<SearchPrefixPosting>,
    pub substrings: Vec<SearchSubstringPosting>,
    pub fuzzy: Vec<SearchFuzzyPosting>,
    pub content: Vec<ContentPosting>,
    pub report: SidecarQueryImportReport,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SidecarQueryImportReport {
    pub metadata_postings: usize,
    pub prefix_postings: usize,
    pub substring_postings: usize,
    pub fuzzy_postings: usize,
    pub content_postings: usize,
    pub candidate_ids: usize,
    pub requires_full_record_hydration: bool,
}

impl SidecarQueryImport {
    pub fn candidate_ids_cancellable(
        &self,
        cancellation: &Cancellation,
    ) -> Result<BTreeSet<FileId>> {
        let mut ids = BTreeSet::new();
        for posting in &self.metadata {
            cancellation.check()?;
            for id in &posting.ids {
                cancellation.check()?;
                ids.insert(*id);
            }
        }
        for posting in &self.prefixes {
            cancellation.check()?;
            for id in &posting.ids {
                cancellation.check()?;
                ids.insert(*id);
            }
        }
        for posting in &self.substrings {
            cancellation.check()?;
            for id in &posting.ids {
                cancellation.check()?;
                ids.insert(*id);
            }
        }
        for posting in &self.content {
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
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SidecarRecordHydrationReport {
    pub records_loaded: usize,
    pub records_missing: usize,
    pub columns_applied: usize,
    pub metadata_keys: usize,
    pub prefix_keys: usize,
    pub substring_keys: usize,
    pub fuzzy_keys: usize,
    pub content_keys: usize,
    pub import: SidecarQueryImportReport,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContentQueryLoadReport {
    pub content_keys: usize,
    pub candidate_ids: usize,
    pub records_loaded: usize,
    pub records_missing: usize,
    pub full_hydration: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarQuerySessionReport {
    pub hydration: SidecarRecordHydrationReport,
    pub search: SearchQueryReport,
    pub content_cache_hits: usize,
    pub content_cache_misses: usize,
    pub record_cache_hits: usize,
    pub record_cache_misses: usize,
    pub result_cache_hits: usize,
    pub result_cache_misses: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarQueryCacheInvalidationReport {
    pub path: std::path::PathBuf,
    pub invalidated: bool,
    pub result_entries_before: usize,
    pub result_entries_after: usize,
    pub reason: String,
}

impl SidecarQueryCacheInvalidationReport {
    pub fn as_tsv(&self) -> String {
        format!(
            "sidecar-query-cache-invalidation\t{}\tinvalidated={}\tresult-entries-before={}\tresult-entries-after={}\treason={}",
            escape_tsv_field(&self.path.to_string_lossy()),
            self.invalidated,
            self.result_entries_before,
            self.result_entries_after,
            escape_tsv_field(&self.reason)
        )
    }
}

fn escape_tsv_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

#[derive(Debug)]
pub struct SidecarIndexQuerySession {
    records: MmapRecordArchive,
    columns: MmapRecordColumns,
    metadata: MmapMetadataArchive,
    lookup: SearchArchiveLookup,
    content: MmapContentArchive,
    content_cache: Mutex<LookupCache<Option<ContentPosting>>>,
    content_cache_hits: AtomicUsize,
    content_cache_misses: AtomicUsize,
    record_cache: Mutex<RecordCache>,
    record_cache_hits: AtomicUsize,
    record_cache_misses: AtomicUsize,
    result_cache: Mutex<LookupCache<SidecarQuerySessionReport>>,
    result_cache_hits: AtomicUsize,
    result_cache_misses: AtomicUsize,
}

impl SidecarIndexQuerySession {
    pub fn open(
        records: impl AsRef<Path>,
        columns: impl AsRef<Path>,
        metadata: impl AsRef<Path>,
        prefixes: impl AsRef<Path>,
        substrings: impl AsRef<Path>,
        fuzzy: impl AsRef<Path>,
        content: impl AsRef<Path>,
    ) -> Result<Self> {
        Self::open_cancellable(
            records,
            columns,
            metadata,
            prefixes,
            substrings,
            fuzzy,
            content,
            &Cancellation::default(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn open_cancellable(
        records: impl AsRef<Path>,
        columns: impl AsRef<Path>,
        metadata: impl AsRef<Path>,
        prefixes: impl AsRef<Path>,
        substrings: impl AsRef<Path>,
        fuzzy: impl AsRef<Path>,
        content: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<Self> {
        cancellation.check()?;
        let substrings = substrings.as_ref();
        let records = MmapRecordArchive::open_checked(records, || cancellation.check())?;
        cancellation.check()?;
        let columns = MmapRecordColumns::open_checked(columns, || cancellation.check())?;
        cancellation.check()?;
        let metadata = MmapMetadataArchive::open_checked(metadata, || cancellation.check())?;
        cancellation.check()?;
        let lookup =
            SearchArchiveLookup::open_cancellable(prefixes, substrings, fuzzy, cancellation)?;
        cancellation.check()?;
        let content = MmapContentArchive::open_checked(content, || cancellation.check())?;
        cancellation.check()?;
        Ok(Self {
            records,
            columns,
            metadata,
            lookup,
            content,
            content_cache: Mutex::new(LookupCache::new(SIDECAR_CONTENT_POSTING_CACHE_CAPACITY)),
            content_cache_hits: AtomicUsize::new(0),
            content_cache_misses: AtomicUsize::new(0),
            record_cache: Mutex::new(RecordCache::new(SIDECAR_RECORD_CACHE_CAPACITY)),
            record_cache_hits: AtomicUsize::new(0),
            record_cache_misses: AtomicUsize::new(0),
            result_cache: Mutex::new(LookupCache::new(SIDECAR_QUERY_RESULT_CACHE_CAPACITY)),
            result_cache_hits: AtomicUsize::new(0),
            result_cache_misses: AtomicUsize::new(0),
        })
    }

    pub fn indexed_records(&self) -> usize {
        self.records.len()
    }

    pub fn indexed_columns(&self) -> usize {
        self.columns.len()
    }

    pub fn indexed_prefixes(&self) -> usize {
        self.lookup.indexed_prefixes()
    }

    pub fn indexed_substring_grams(&self) -> usize {
        self.lookup.indexed_substring_grams()
    }

    pub fn indexed_fuzzy_keys(&self) -> usize {
        self.lookup.indexed_fuzzy_keys()
    }

    pub fn lookup_telemetry(&self) -> SearchLookupTelemetry {
        self.lookup.cache_telemetry()
    }

    pub fn record_cache_telemetry(&self) -> (usize, usize) {
        (
            self.record_cache_hits.load(Ordering::Relaxed),
            self.record_cache_misses.load(Ordering::Relaxed),
        )
    }

    pub fn content_cache_telemetry(&self) -> (usize, usize) {
        (
            self.content_cache_hits.load(Ordering::Relaxed),
            self.content_cache_misses.load(Ordering::Relaxed),
        )
    }

    pub fn result_cache_telemetry(&self) -> (usize, usize) {
        (
            self.result_cache_hits.load(Ordering::Relaxed),
            self.result_cache_misses.load(Ordering::Relaxed),
        )
    }

    pub fn apply_provider_metadata_invalidation(
        &self,
        report: &crate::ProviderMetadataInvalidationReport,
    ) -> SidecarQueryCacheInvalidationReport {
        let mut cache = self.result_cache_lock();
        let result_entries_before = cache.len();
        if report.invalidate_query_cache {
            cache.clear();
        }
        let result_entries_after = cache.len();
        SidecarQueryCacheInvalidationReport {
            path: report.path.clone(),
            invalidated: report.invalidate_query_cache,
            result_entries_before,
            result_entries_after,
            reason: report.reason.clone(),
        }
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<SidecarQuerySessionReport> {
        self.search_with_budget(query, limit, SearchLookupBudget::default())
    }

    pub fn search_with_budget(
        &self,
        query: &str,
        limit: usize,
        budget: SearchLookupBudget,
    ) -> Result<SidecarQuerySessionReport> {
        self.search_structured_with_volume_scope_budget_cancellable(
            &SearchQuery::parse(query),
            limit,
            &SearchVolumeScope::All,
            budget,
            &Cancellation::default(),
        )
    }

    pub fn search_cancellable(
        &self,
        query: &str,
        limit: usize,
        cancellation: &Cancellation,
    ) -> Result<SidecarQuerySessionReport> {
        self.search_with_volume_scope_budget_cancellable(
            query,
            limit,
            &SearchVolumeScope::All,
            SearchLookupBudget::default(),
            cancellation,
        )
    }

    pub fn search_with_budget_cancellable(
        &self,
        query: &str,
        limit: usize,
        budget: SearchLookupBudget,
        cancellation: &Cancellation,
    ) -> Result<SidecarQuerySessionReport> {
        self.search_with_volume_scope_budget_cancellable(
            query,
            limit,
            &SearchVolumeScope::All,
            budget,
            cancellation,
        )
    }

    pub fn search_with_volume_scope(
        &self,
        query: &str,
        limit: usize,
        scope: &SearchVolumeScope,
    ) -> Result<SidecarQuerySessionReport> {
        self.search_with_volume_scope_budget_cancellable(
            query,
            limit,
            scope,
            SearchLookupBudget::default(),
            &Cancellation::default(),
        )
    }

    pub fn search_with_volume_scope_budget_cancellable(
        &self,
        query: &str,
        limit: usize,
        scope: &SearchVolumeScope,
        budget: SearchLookupBudget,
        cancellation: &Cancellation,
    ) -> Result<SidecarQuerySessionReport> {
        let parsed = SearchQuery::parse_cancellable(query, cancellation)?;
        self.search_structured_with_volume_scope_budget_cancellable(
            &parsed,
            limit,
            scope,
            budget,
            cancellation,
        )
    }

    pub fn search_structured_with_volume_scope_budget_cancellable(
        &self,
        parsed: &SearchQuery,
        limit: usize,
        scope: &SearchVolumeScope,
        budget: SearchLookupBudget,
        cancellation: &Cancellation,
    ) -> Result<SidecarQuerySessionReport> {
        cancellation.check()?;
        if parsed.is_empty() || limit == 0 || scope_excludes_all(scope) {
            return Ok(empty_sidecar_query_session_report());
        }
        let result_cache_key = query_result_cache_key(parsed, limit, scope, budget);
        if let Some(mut report) = self.result_cache_lock().get(&result_cache_key) {
            self.result_cache_hits.fetch_add(1, Ordering::Relaxed);
            report.search.lookup = SearchLookupTelemetry::default();
            report.content_cache_hits = 0;
            report.content_cache_misses = 0;
            report.record_cache_hits = 0;
            report.record_cache_misses = 0;
            report.result_cache_hits = 1;
            report.result_cache_misses = 0;
            return Ok(report);
        }
        self.result_cache_misses.fetch_add(1, Ordering::Relaxed);
        let content_hits_before = self.content_cache_hits.load(Ordering::Relaxed);
        let content_misses_before = self.content_cache_misses.load(Ordering::Relaxed);
        cancellation.check()?;
        let content_terms = parsed.content_candidate_terms_cancellable(cancellation)?;
        let content_postings = self.scoped_content_postings_for_terms(
            content_terms.clone(),
            budget.max_content_ids_per_term,
            scope,
            cancellation,
        )?;
        cancellation.check()?;
        let import = query_sidecar_imports_with_content_postings_scoped(
            SidecarImportSources {
                metadata: &self.metadata,
                lookup: &self.lookup,
            },
            parsed,
            SidecarContentImport {
                terms: content_terms,
                postings: content_postings,
            },
            budget,
            scope,
            cancellation,
        )?;
        cancellation.check()?;
        let cache_hits_before = self.record_cache_hits.load(Ordering::Relaxed);
        let cache_misses_before = self.record_cache_misses.load(Ordering::Relaxed);
        let (live, hydration) = self.live_from_import(import, cancellation)?;
        let search = live.search_structured_with_volume_scope_lookup_budget_cancellable(
            parsed,
            limit,
            scope,
            &self.lookup,
            budget,
            cancellation,
        )?;
        let report = SidecarQuerySessionReport {
            hydration,
            search,
            content_cache_hits: self
                .content_cache_hits
                .load(Ordering::Relaxed)
                .saturating_sub(content_hits_before),
            content_cache_misses: self
                .content_cache_misses
                .load(Ordering::Relaxed)
                .saturating_sub(content_misses_before),
            record_cache_hits: self
                .record_cache_hits
                .load(Ordering::Relaxed)
                .saturating_sub(cache_hits_before),
            record_cache_misses: self
                .record_cache_misses
                .load(Ordering::Relaxed)
                .saturating_sub(cache_misses_before),
            result_cache_hits: 0,
            result_cache_misses: 1,
        };
        self.result_cache_lock()
            .insert(result_cache_key, report.clone());
        Ok(report)
    }

    fn content_postings_for_terms(
        &self,
        terms: Vec<String>,
        limit_per_term: usize,
        cancellation: &Cancellation,
    ) -> Result<Vec<ContentPosting>> {
        if limit_per_term == 0 {
            return Ok(Vec::new());
        }

        let mut selected = BTreeSet::new();
        for term in terms {
            cancellation.check()?;
            let term = canonical_content_query_term_checked(&term, || cancellation.check())?;
            if !term.is_empty() {
                selected.insert(term);
            }
        }
        if selected.is_empty() {
            return Ok(Vec::new());
        }

        let mut postings = Vec::with_capacity(selected.len());
        let mut misses = Vec::new();
        {
            let mut cache = self.content_cache_lock();
            for term in &selected {
                cancellation.check()?;
                let key = bounded_posting_cache_key(term, limit_per_term);
                if let Some(cached) = cache.get(&key) {
                    self.content_cache_hits.fetch_add(1, Ordering::Relaxed);
                    if let Some(posting) = cached {
                        postings.push(posting);
                    }
                } else {
                    self.content_cache_misses.fetch_add(1, Ordering::Relaxed);
                    misses.push(term.clone());
                }
            }
        }

        cancellation.check()?;
        let loaded = self
            .content
            .postings_for_sorted_terms_limit_checked(&misses, limit_per_term, || {
                cancellation.check()
            })?
            .into_iter()
            .map(|limited| {
                (
                    limited.posting.term.clone(),
                    (Some(limited.posting), limited.truncated),
                )
            })
            .collect::<HashMap<_, _>>();

        let mut cache = self.content_cache_lock();
        for term in misses {
            cancellation.check()?;
            let (posting, truncated) = loaded.get(&term).cloned().unwrap_or((None, false));
            if !truncated {
                cache.insert(
                    bounded_posting_cache_key(&term, limit_per_term),
                    posting.clone(),
                );
            }
            if let Some(posting) = posting {
                postings.push(posting);
            }
        }

        postings.sort_by(|left, right| left.term.cmp(&right.term));
        Ok(postings)
    }

    fn scoped_content_postings_for_terms(
        &self,
        terms: Vec<String>,
        limit_per_term: usize,
        scope: &SearchVolumeScope,
        cancellation: &Cancellation,
    ) -> Result<Vec<ContentPosting>> {
        if scope_excludes_all(scope) || !self.records_contains_scope(scope) {
            return Ok(Vec::new());
        }
        match scope {
            SearchVolumeScope::All => {
                self.content_postings_for_terms(terms, limit_per_term, cancellation)
            }
            SearchVolumeScope::Only(volumes) => {
                let mut postings = Vec::new();
                for volume in volumes {
                    cancellation.check()?;
                    if self.records.contains_volume(*volume) {
                        postings.extend(self.content_postings_for_terms_in_volume(
                            terms.clone(),
                            *volume,
                            limit_per_term,
                            cancellation,
                        )?);
                    }
                }
                Ok(postings)
            }
        }
    }

    fn records_contains_scope(&self, scope: &SearchVolumeScope) -> bool {
        match scope {
            SearchVolumeScope::All => !self.records.is_empty(),
            SearchVolumeScope::Only(volumes) => volumes
                .iter()
                .any(|volume| self.records.contains_volume(*volume)),
        }
    }

    fn content_postings_for_terms_in_volume(
        &self,
        terms: Vec<String>,
        volume: VolumeId,
        limit_per_term: usize,
        cancellation: &Cancellation,
    ) -> Result<Vec<ContentPosting>> {
        if limit_per_term == 0 {
            return Ok(Vec::new());
        }

        let mut selected = BTreeSet::new();
        for term in terms {
            cancellation.check()?;
            let term = canonical_content_query_term_checked(&term, || cancellation.check())?;
            if !term.is_empty() {
                selected.insert(term);
            }
        }
        if selected.is_empty() {
            return Ok(Vec::new());
        }

        let mut postings = Vec::with_capacity(selected.len());
        let mut misses = Vec::new();
        {
            let mut cache = self.content_cache_lock();
            for term in &selected {
                cancellation.check()?;
                let key = bounded_volume_posting_cache_key(term, volume, limit_per_term);
                if let Some(cached) = cache.get(&key) {
                    self.content_cache_hits.fetch_add(1, Ordering::Relaxed);
                    if let Some(posting) = cached {
                        postings.push(posting);
                    }
                } else {
                    self.content_cache_misses.fetch_add(1, Ordering::Relaxed);
                    misses.push(term.clone());
                }
            }
        }

        cancellation.check()?;
        let loaded = self
            .content
            .postings_for_sorted_terms_volume_limit_checked(
                &misses,
                volume,
                limit_per_term,
                || cancellation.check(),
            )?
            .into_iter()
            .map(|limited| {
                (
                    limited.posting.term.clone(),
                    (Some(limited.posting), limited.truncated),
                )
            })
            .collect::<HashMap<_, _>>();

        let mut cache = self.content_cache_lock();
        for term in misses {
            cancellation.check()?;
            let (posting, truncated) = loaded.get(&term).cloned().unwrap_or((None, false));
            if !truncated {
                cache.insert(
                    bounded_volume_posting_cache_key(&term, volume, limit_per_term),
                    posting.clone(),
                );
            }
            if let Some(posting) = posting {
                postings.push(posting);
            }
        }

        postings.sort_by(|left, right| {
            left.term
                .cmp(&right.term)
                .then_with(|| left.ids.first().cmp(&right.ids.first()))
        });
        Ok(postings)
    }

    fn live_from_import(
        &self,
        import: SidecarQueryImport,
        cancellation: &Cancellation,
    ) -> Result<(crate::LiveIndex, SidecarRecordHydrationReport)> {
        cancellation.check()?;
        let (records, missing) = if import.report.requires_full_record_hydration {
            self.hydrate_all_records(cancellation)?
        } else {
            self.hydrate_record_ids(
                sidecar_candidate_ids_cancellable(&import, cancellation)?,
                cancellation,
            )?
        };
        cancellation.check()?;
        let (live, applied, metadata_keys, prefix_keys, substring_keys, fuzzy_keys, content_keys) =
            crate::LiveIndex::from_records_with_sidecars(
                records
                    .iter()
                    .map(|record| record.record.clone())
                    .collect::<Vec<_>>(),
                records
                    .into_iter()
                    .filter_map(|record| record.columns)
                    .collect::<Vec<_>>(),
                import.metadata,
                import.prefixes,
                import.substrings,
                import.fuzzy,
                import.content,
            );
        let report = SidecarRecordHydrationReport {
            records_loaded: live.indexed_records(),
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

    fn hydrate_all_records(
        &self,
        cancellation: &Cancellation,
    ) -> Result<(Vec<HydratedRecord>, usize)> {
        let mut records = Vec::with_capacity(self.records.len());
        for index in 0..self.records.len() {
            cancellation.check()?;
            let record = self.records.record(index)?;
            records.push(self.hydrate_record_checked(record, cancellation)?);
        }
        Ok((records, 0))
    }

    fn hydrate_record_ids(
        &self,
        ids: BTreeSet<FileId>,
        cancellation: &Cancellation,
    ) -> Result<(Vec<HydratedRecord>, usize)> {
        if ids.is_empty() {
            return Ok((Vec::new(), 0));
        }

        let mut hydrated_by_id = HashMap::new();
        let mut misses = Vec::new();
        {
            let cache = self.record_cache_lock();
            for id in &ids {
                cancellation.check()?;
                if let Some(record) = cache.get(*id) {
                    self.record_cache_hits.fetch_add(1, Ordering::Relaxed);
                    hydrated_by_id.insert(*id, record);
                } else {
                    self.record_cache_misses.fetch_add(1, Ordering::Relaxed);
                    misses.push(*id);
                }
            }
        }

        cancellation.check()?;
        let batch = self
            .records
            .records_for_sorted_ids_checked(misses.iter().copied(), || cancellation.check())?;
        let missing = batch.missing;
        let mut loaded = Vec::with_capacity(batch.records.len());
        for record in batch.records {
            cancellation.check()?;
            let id = record.id;
            loaded.push((id, self.hydrate_record_checked(record, cancellation)?));
        }
        {
            let mut cache = self.record_cache_lock();
            for (id, hydrated) in loaded {
                cache.insert(id, hydrated.clone());
                hydrated_by_id.insert(id, hydrated);
            }
        }

        let records = ids
            .into_iter()
            .filter_map(|id| hydrated_by_id.remove(&id))
            .collect::<Vec<_>>();
        Ok((records, missing))
    }

    fn content_cache_lock(&self) -> MutexGuard<'_, LookupCache<Option<ContentPosting>>> {
        self.content_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn record_cache_lock(&self) -> MutexGuard<'_, RecordCache> {
        self.record_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn result_cache_lock(&self) -> MutexGuard<'_, LookupCache<SidecarQuerySessionReport>> {
        self.result_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn hydrate_record_checked(
        &self,
        record: FileRecord,
        cancellation: &Cancellation,
    ) -> Result<HydratedRecord> {
        cancellation.check()?;
        let columns = self
            .columns
            .find_checked(record.id, || cancellation.check())?
            .map(|column| SearchRecordColumns {
                id: column.id,
                name: column.name,
                path: column.path,
                extension: column.extension,
                tags: column.tags,
                comment: column.comment,
            });
        cancellation.check()?;
        Ok(HydratedRecord { record, columns })
    }
}

#[derive(Debug, Clone)]
struct HydratedRecord {
    record: FileRecord,
    columns: Option<SearchRecordColumns>,
}

#[derive(Debug)]
pub struct SearchArchiveLookup {
    prefixes: MmapPrefixArchive,
    substrings: MmapSubstringArchive,
    fuzzy: MmapFuzzyArchive,
    prefix_cache: Mutex<LookupCache<Vec<FileId>>>,
    substring_cache: Mutex<LookupCache<Vec<FileId>>>,
    fuzzy_cache: Mutex<LookupCache<Vec<String>>>,
    prefix_requests: AtomicUsize,
    prefix_hits: AtomicUsize,
    prefix_misses: AtomicUsize,
    substring_requests: AtomicUsize,
    substring_hits: AtomicUsize,
    substring_misses: AtomicUsize,
    fuzzy_requests: AtomicUsize,
    fuzzy_hits: AtomicUsize,
    fuzzy_misses: AtomicUsize,
}

impl SearchArchiveLookup {
    pub fn open(
        prefixes: impl AsRef<Path>,
        substrings: impl AsRef<Path>,
        fuzzy: impl AsRef<Path>,
    ) -> Result<SearchArchiveLookup> {
        Self::open_cancellable(prefixes, substrings, fuzzy, &Cancellation::default())
    }

    pub fn open_cancellable(
        prefixes: impl AsRef<Path>,
        substrings: impl AsRef<Path>,
        fuzzy: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<SearchArchiveLookup> {
        Self::open_with_capacity_cancellable(
            prefixes,
            substrings,
            fuzzy,
            SEARCH_ARCHIVE_LOOKUP_CACHE_CAPACITY,
            cancellation,
        )
    }

    #[allow(dead_code)]
    fn open_with_capacity(
        prefixes: impl AsRef<Path>,
        substrings: impl AsRef<Path>,
        fuzzy: impl AsRef<Path>,
        cache_capacity: usize,
    ) -> Result<SearchArchiveLookup> {
        Self::open_with_capacity_cancellable(
            prefixes,
            substrings,
            fuzzy,
            cache_capacity,
            &Cancellation::default(),
        )
    }

    fn open_with_capacity_cancellable(
        prefixes: impl AsRef<Path>,
        substrings: impl AsRef<Path>,
        fuzzy: impl AsRef<Path>,
        cache_capacity: usize,
        cancellation: &Cancellation,
    ) -> Result<SearchArchiveLookup> {
        cancellation.check()?;
        let prefixes = MmapPrefixArchive::open_checked(prefixes, || cancellation.check())?;
        cancellation.check()?;
        let substrings = MmapSubstringArchive::open_checked(substrings, || cancellation.check())?;
        cancellation.check()?;
        let fuzzy = MmapFuzzyArchive::open_checked(fuzzy, || cancellation.check())?;
        cancellation.check()?;
        Ok(Self {
            prefixes,
            substrings,
            fuzzy,
            prefix_cache: Mutex::new(LookupCache::new(cache_capacity)),
            substring_cache: Mutex::new(LookupCache::new(cache_capacity)),
            fuzzy_cache: Mutex::new(LookupCache::new(cache_capacity)),
            prefix_requests: AtomicUsize::new(0),
            prefix_hits: AtomicUsize::new(0),
            prefix_misses: AtomicUsize::new(0),
            substring_requests: AtomicUsize::new(0),
            substring_hits: AtomicUsize::new(0),
            substring_misses: AtomicUsize::new(0),
            fuzzy_requests: AtomicUsize::new(0),
            fuzzy_hits: AtomicUsize::new(0),
            fuzzy_misses: AtomicUsize::new(0),
        })
    }

    #[cfg(test)]
    pub(crate) fn open_with_cache_capacity(
        prefixes: impl AsRef<Path>,
        substrings: impl AsRef<Path>,
        fuzzy: impl AsRef<Path>,
        cache_capacity: usize,
    ) -> Result<SearchArchiveLookup> {
        Self::open_with_capacity(prefixes, substrings, fuzzy, cache_capacity)
    }

    pub fn indexed_prefixes(&self) -> usize {
        self.prefixes.indexed_prefixes()
    }

    pub fn indexed_substring_grams(&self) -> usize {
        self.substrings.indexed_grams()
    }

    pub fn indexed_fuzzy_keys(&self) -> usize {
        self.fuzzy.indexed_keys()
    }

    #[cfg(test)]
    pub(crate) fn cache_entry_counts(&self) -> Result<(usize, usize, usize)> {
        let prefixes = self.prefix_cache_lock().len();
        let substrings = self.substring_cache_lock().len();
        let fuzzy = self.fuzzy_cache_lock().len();
        Ok((prefixes, substrings, fuzzy))
    }

    fn prefix_postings_bounded_cancellable<I, S>(
        &self,
        prefixes: I,
        limit: usize,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchPrefixPosting>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut selected = BTreeSet::new();
        for prefix in prefixes {
            cancellation.check()?;
            let prefix = prefix.as_ref();
            if !prefix.is_empty() {
                selected.insert(prefix.to_string());
            }
        }

        let mut postings = Vec::with_capacity(selected.len());
        let mut misses = Vec::new();
        {
            let mut cache = self.prefix_cache_lock();
            for prefix in &selected {
                cancellation.check()?;
                self.prefix_requests.fetch_add(1, Ordering::Relaxed);
                if let Some(mut ids) = cache.get(prefix) {
                    self.prefix_hits.fetch_add(1, Ordering::Relaxed);
                    ids.truncate(limit);
                    postings.push(SearchPrefixPosting {
                        prefix: prefix.clone(),
                        ids,
                    });
                } else {
                    self.prefix_misses.fetch_add(1, Ordering::Relaxed);
                    misses.push(prefix.clone());
                }
            }
        }

        cancellation.check()?;
        let loaded = self
            .prefixes
            .postings_for_sorted_prefixes_limit_checked(&misses, limit, || cancellation.check())?
            .into_iter()
            .map(|limited| {
                (
                    limited.posting.prefix,
                    (limited.posting.ids, limited.truncated),
                )
            })
            .collect::<HashMap<_, _>>();

        let mut cache = self.prefix_cache_lock();
        for prefix in misses {
            cancellation.check()?;
            let (ids, truncated) = loaded
                .get(&prefix)
                .cloned()
                .unwrap_or_else(|| (Vec::new(), false));
            if !truncated {
                cache.insert(prefix.clone(), ids.clone());
            }
            postings.push(SearchPrefixPosting { prefix, ids });
        }

        postings.sort_by(|left, right| left.prefix.cmp(&right.prefix));
        Ok(postings)
    }

    #[cfg(test)]
    pub(crate) fn fuzzy_postings_bounded<I, S>(
        &self,
        keys: I,
        limit: usize,
    ) -> Result<Vec<SearchFuzzyPosting>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.fuzzy_postings_bounded_cancellable(keys, limit, &Cancellation::default())
    }

    pub(crate) fn fuzzy_postings_bounded_cancellable<I, S>(
        &self,
        keys: I,
        limit: usize,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchFuzzyPosting>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut selected = BTreeSet::new();
        for key in keys {
            cancellation.check()?;
            let key = key.as_ref();
            if !key.is_empty() {
                selected.insert(key.to_string());
            }
        }

        let mut postings = Vec::with_capacity(selected.len());
        let mut misses = Vec::new();
        {
            let mut cache = self.fuzzy_cache_lock();
            for key in &selected {
                cancellation.check()?;
                self.fuzzy_requests.fetch_add(1, Ordering::Relaxed);
                if let Some(mut terms) = cache.get(key) {
                    self.fuzzy_hits.fetch_add(1, Ordering::Relaxed);
                    terms.truncate(limit);
                    postings.push(SearchFuzzyPosting {
                        key: key.clone(),
                        terms,
                    });
                } else {
                    self.fuzzy_misses.fetch_add(1, Ordering::Relaxed);
                    misses.push(key.clone());
                }
            }
        }

        cancellation.check()?;
        let loaded = self
            .fuzzy
            .postings_for_sorted_keys_limit_checked(&misses, limit, || cancellation.check())?
            .into_iter()
            .map(|limited| {
                (
                    limited.posting.key,
                    (limited.posting.terms, limited.truncated),
                )
            })
            .collect::<HashMap<_, _>>();

        let mut cache = self.fuzzy_cache_lock();
        for key in misses {
            cancellation.check()?;
            let (terms, truncated) = loaded
                .get(&key)
                .cloned()
                .unwrap_or_else(|| (Vec::new(), false));
            if !truncated {
                cache.insert(key.clone(), terms.clone());
            }
            postings.push(SearchFuzzyPosting { key, terms });
        }

        postings.sort_by(|left, right| left.key.cmp(&right.key));
        Ok(postings)
    }

    fn prefix_ids_for_volume_bounded_cancellable(
        &self,
        prefix: &str,
        volume: VolumeId,
        limit: usize,
        cancellation: &Cancellation,
    ) -> Result<SearchLookupIds> {
        self.prefix_requests.fetch_add(1, Ordering::Relaxed);
        if limit == 0 {
            return Ok(SearchLookupIds::new(Vec::new(), false));
        }
        let cache_key = volume_cache_key(prefix, volume);
        if let Some(mut ids) = self.prefix_cache_lock().get(&cache_key) {
            self.prefix_hits.fetch_add(1, Ordering::Relaxed);
            let truncated = ids.len() > limit;
            ids.truncate(limit);
            return Ok(SearchLookupIds::new(ids, truncated));
        }
        cancellation.check()?;
        self.prefix_misses.fetch_add(1, Ordering::Relaxed);
        let (ids, truncated) =
            self.prefixes
                .ids_for_volume_limit_checked(prefix, volume, limit, || cancellation.check())?;
        if !truncated {
            self.prefix_cache_lock().insert(cache_key, ids.clone());
        }
        Ok(SearchLookupIds::new(ids, truncated))
    }

    fn substring_ids_for_volume_bounded_cancellable(
        &self,
        gram: &str,
        volume: VolumeId,
        limit: usize,
        cancellation: &Cancellation,
    ) -> Result<SearchLookupIds> {
        self.substring_requests.fetch_add(1, Ordering::Relaxed);
        if limit == 0 {
            return Ok(SearchLookupIds::new(Vec::new(), false));
        }
        let cache_key = volume_cache_key(gram, volume);
        if let Some(mut ids) = self.substring_cache_lock().get(&cache_key) {
            self.substring_hits.fetch_add(1, Ordering::Relaxed);
            let truncated = ids.len() > limit;
            ids.truncate(limit);
            return Ok(SearchLookupIds::new(ids, truncated));
        }
        cancellation.check()?;
        self.substring_misses.fetch_add(1, Ordering::Relaxed);
        let (ids, truncated) =
            self.substrings
                .ids_for_volume_limit_checked(gram, volume, limit, || cancellation.check())?;
        if !truncated {
            self.substring_cache_lock().insert(cache_key, ids.clone());
        }
        Ok(SearchLookupIds::new(ids, truncated))
    }

    fn prefix_cache_lock(&self) -> MutexGuard<'_, LookupCache<Vec<FileId>>> {
        self.prefix_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn substring_cache_lock(&self) -> MutexGuard<'_, LookupCache<Vec<FileId>>> {
        self.substring_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn fuzzy_cache_lock(&self) -> MutexGuard<'_, LookupCache<Vec<String>>> {
        self.fuzzy_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl SearchLookup for SearchArchiveLookup {
    fn prefix_ids(&self, prefix: &str) -> Result<Vec<FileId>> {
        self.prefix_requests.fetch_add(1, Ordering::Relaxed);
        if let Some(ids) = self.prefix_cache_lock().get(prefix) {
            self.prefix_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(ids);
        }

        self.prefix_misses.fetch_add(1, Ordering::Relaxed);
        let ids = self.prefixes.ids_for(prefix)?;
        self.prefix_cache_lock()
            .insert(prefix.to_string(), ids.clone());
        Ok(ids)
    }

    fn prefix_ids_bounded(&self, prefix: &str, limit: usize) -> Result<SearchLookupIds> {
        self.prefix_requests.fetch_add(1, Ordering::Relaxed);
        if limit == 0 {
            return Ok(SearchLookupIds::new(Vec::new(), false));
        }
        if let Some(mut ids) = self.prefix_cache_lock().get(prefix) {
            self.prefix_hits.fetch_add(1, Ordering::Relaxed);
            let truncated = ids.len() > limit;
            ids.truncate(limit);
            return Ok(SearchLookupIds::new(ids, truncated));
        }

        self.prefix_misses.fetch_add(1, Ordering::Relaxed);
        let (ids, truncated) = self.prefixes.ids_for_limit(prefix, limit)?;
        if !truncated {
            self.prefix_cache_lock()
                .insert(prefix.to_string(), ids.clone());
        }
        Ok(SearchLookupIds::new(ids, truncated))
    }

    fn prefix_ids_for_volume(&self, prefix: &str, volume: VolumeId) -> Result<Vec<FileId>> {
        self.prefix_requests.fetch_add(1, Ordering::Relaxed);
        self.prefix_misses.fetch_add(1, Ordering::Relaxed);
        let (ids, _) = self
            .prefixes
            .ids_for_volume_limit(prefix, volume, usize::MAX)?;
        Ok(ids)
    }

    fn prefix_ids_for_volume_bounded(
        &self,
        prefix: &str,
        volume: VolumeId,
        limit: usize,
    ) -> Result<SearchLookupIds> {
        self.prefix_requests.fetch_add(1, Ordering::Relaxed);
        if limit == 0 {
            return Ok(SearchLookupIds::new(Vec::new(), false));
        }
        let cache_key = volume_cache_key(prefix, volume);
        if let Some(mut ids) = self.prefix_cache_lock().get(&cache_key) {
            self.prefix_hits.fetch_add(1, Ordering::Relaxed);
            let truncated = ids.len() > limit;
            ids.truncate(limit);
            return Ok(SearchLookupIds::new(ids, truncated));
        }
        self.prefix_misses.fetch_add(1, Ordering::Relaxed);
        let (ids, truncated) = self.prefixes.ids_for_volume_limit(prefix, volume, limit)?;
        if !truncated {
            self.prefix_cache_lock().insert(cache_key, ids.clone());
        }
        Ok(SearchLookupIds::new(ids, truncated))
    }

    fn substring_ids(&self, gram: &str) -> Result<Vec<FileId>> {
        self.substring_requests.fetch_add(1, Ordering::Relaxed);
        if let Some(ids) = self.substring_cache_lock().get(gram) {
            self.substring_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(ids);
        }

        self.substring_misses.fetch_add(1, Ordering::Relaxed);
        let ids = self.substrings.ids_for(gram)?;
        self.substring_cache_lock()
            .insert(gram.to_string(), ids.clone());
        Ok(ids)
    }

    fn substring_ids_bounded(&self, gram: &str, limit: usize) -> Result<SearchLookupIds> {
        self.substring_requests.fetch_add(1, Ordering::Relaxed);
        if limit == 0 {
            return Ok(SearchLookupIds::new(Vec::new(), false));
        }
        if let Some(mut ids) = self.substring_cache_lock().get(gram) {
            self.substring_hits.fetch_add(1, Ordering::Relaxed);
            let truncated = ids.len() > limit;
            ids.truncate(limit);
            return Ok(SearchLookupIds::new(ids, truncated));
        }

        self.substring_misses.fetch_add(1, Ordering::Relaxed);
        let (ids, truncated) = self.substrings.ids_for_limit(gram, limit)?;
        if !truncated {
            self.substring_cache_lock()
                .insert(gram.to_string(), ids.clone());
        }
        Ok(SearchLookupIds::new(ids, truncated))
    }

    fn substring_ids_for_volume(&self, gram: &str, volume: VolumeId) -> Result<Vec<FileId>> {
        self.substring_requests.fetch_add(1, Ordering::Relaxed);
        self.substring_misses.fetch_add(1, Ordering::Relaxed);
        let (ids, _) = self
            .substrings
            .ids_for_volume_limit(gram, volume, usize::MAX)?;
        Ok(ids)
    }

    fn substring_ids_for_volume_bounded(
        &self,
        gram: &str,
        volume: VolumeId,
        limit: usize,
    ) -> Result<SearchLookupIds> {
        self.substring_requests.fetch_add(1, Ordering::Relaxed);
        if limit == 0 {
            return Ok(SearchLookupIds::new(Vec::new(), false));
        }
        let cache_key = volume_cache_key(gram, volume);
        if let Some(mut ids) = self.substring_cache_lock().get(&cache_key) {
            self.substring_hits.fetch_add(1, Ordering::Relaxed);
            let truncated = ids.len() > limit;
            ids.truncate(limit);
            return Ok(SearchLookupIds::new(ids, truncated));
        }
        self.substring_misses.fetch_add(1, Ordering::Relaxed);
        let (ids, truncated) = self.substrings.ids_for_volume_limit(gram, volume, limit)?;
        if !truncated {
            self.substring_cache_lock().insert(cache_key, ids.clone());
        }
        Ok(SearchLookupIds::new(ids, truncated))
    }

    fn fuzzy_terms(&self, key: &str) -> Result<Vec<String>> {
        self.fuzzy_requests.fetch_add(1, Ordering::Relaxed);
        if let Some(terms) = self.fuzzy_cache_lock().get(key) {
            self.fuzzy_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(terms);
        }

        self.fuzzy_misses.fetch_add(1, Ordering::Relaxed);
        let terms = self.fuzzy.terms_for(key)?;
        self.fuzzy_cache_lock()
            .insert(key.to_string(), terms.clone());
        Ok(terms)
    }

    fn fuzzy_terms_bounded(&self, key: &str, limit: usize) -> Result<SearchLookupTerms> {
        self.fuzzy_requests.fetch_add(1, Ordering::Relaxed);
        if limit == 0 {
            return Ok(SearchLookupTerms::new(Vec::new(), false));
        }
        if let Some(mut terms) = self.fuzzy_cache_lock().get(key) {
            self.fuzzy_hits.fetch_add(1, Ordering::Relaxed);
            let truncated = terms.len() > limit;
            terms.truncate(limit);
            return Ok(SearchLookupTerms::new(terms, truncated));
        }

        self.fuzzy_misses.fetch_add(1, Ordering::Relaxed);
        let (terms, truncated) = self.fuzzy.terms_for_limit(key, limit)?;
        if !truncated {
            self.fuzzy_cache_lock()
                .insert(key.to_string(), terms.clone());
        }
        Ok(SearchLookupTerms::new(terms, truncated))
    }

    fn cache_telemetry(&self) -> SearchLookupTelemetry {
        SearchLookupTelemetry {
            prefix_lookup_requests: self.prefix_requests.load(Ordering::Relaxed),
            prefix_cache_hits: self.prefix_hits.load(Ordering::Relaxed),
            prefix_cache_misses: self.prefix_misses.load(Ordering::Relaxed),
            substring_lookup_requests: self.substring_requests.load(Ordering::Relaxed),
            substring_cache_hits: self.substring_hits.load(Ordering::Relaxed),
            substring_cache_misses: self.substring_misses.load(Ordering::Relaxed),
            fuzzy_lookup_requests: self.fuzzy_requests.load(Ordering::Relaxed),
            fuzzy_cache_hits: self.fuzzy_hits.load(Ordering::Relaxed),
            fuzzy_cache_misses: self.fuzzy_misses.load(Ordering::Relaxed),
            ..SearchLookupTelemetry::default()
        }
    }
}

fn volume_cache_key(term: &str, volume: VolumeId) -> String {
    format!("volume={}:{}", volume.0, term)
}

pub fn query_sidecar_imports(
    metadata: &MmapMetadataArchive,
    lookup: &SearchArchiveLookup,
    substrings: &MmapSubstringArchive,
    content: &MmapContentArchive,
    query: &str,
    budget: SearchLookupBudget,
) -> Result<SidecarQueryImport> {
    let parsed = gfm_search::SearchQuery::parse(query);
    let content_terms = parsed.content_candidate_terms();
    let content =
        content.postings_for_terms_limit(content_terms.clone(), budget.max_content_ids_per_term)?;
    query_sidecar_imports_with_content_postings(
        metadata,
        lookup,
        substrings,
        &parsed,
        SidecarContentImport {
            terms: content_terms,
            postings: content,
        },
        budget,
        &Cancellation::default(),
    )
}

pub fn query_sidecar_imports_cancellable(
    metadata: &MmapMetadataArchive,
    lookup: &SearchArchiveLookup,
    substrings: &MmapSubstringArchive,
    content: &MmapContentArchive,
    query: &str,
    budget: SearchLookupBudget,
    cancellation: &Cancellation,
) -> Result<SidecarQueryImport> {
    cancellation.check()?;
    let parsed = gfm_search::SearchQuery::parse_cancellable(query, cancellation)?;
    cancellation.check()?;
    let content_terms = parsed.content_candidate_terms_cancellable(cancellation)?;
    let content = content.postings_for_terms_limit_checked(
        content_terms.clone(),
        budget.max_content_ids_per_term,
        || cancellation.check(),
    )?;
    cancellation.check()?;
    query_sidecar_imports_with_content_postings(
        metadata,
        lookup,
        substrings,
        &parsed,
        SidecarContentImport {
            terms: content_terms,
            postings: content,
        },
        budget,
        cancellation,
    )
}

struct SidecarContentImport {
    terms: Vec<String>,
    postings: Vec<ContentPosting>,
}

struct SidecarImportSources<'a> {
    metadata: &'a MmapMetadataArchive,
    lookup: &'a SearchArchiveLookup,
}

fn query_sidecar_imports_with_content_postings(
    metadata: &MmapMetadataArchive,
    lookup: &SearchArchiveLookup,
    _substrings: &MmapSubstringArchive,
    parsed: &gfm_search::SearchQuery,
    content: SidecarContentImport,
    budget: SearchLookupBudget,
    cancellation: &Cancellation,
) -> Result<SidecarQueryImport> {
    query_sidecar_imports_with_content_postings_scoped(
        SidecarImportSources { metadata, lookup },
        parsed,
        content,
        budget,
        &SearchVolumeScope::All,
        cancellation,
    )
}

fn query_sidecar_imports_with_content_postings_scoped(
    sources: SidecarImportSources<'_>,
    parsed: &gfm_search::SearchQuery,
    content: SidecarContentImport,
    budget: SearchLookupBudget,
    scope: &SearchVolumeScope,
    cancellation: &Cancellation,
) -> Result<SidecarQueryImport> {
    cancellation.check()?;
    if scope_excludes_all(scope) {
        return Ok(SidecarQueryImport::default());
    }
    let comment_terms = parsed.comment_candidate_terms_cancellable(cancellation)?;
    let tag_terms = parsed.tag_candidate_terms_cancellable(cancellation)?;
    let prefix_terms = parsed.prefix_candidate_terms_cancellable(cancellation)?;
    let substring_grams = bounded_substring_grams(&content.terms, budget);
    let fuzzy_keys = parsed
        .fuzzy_candidate_keys_cancellable(cancellation)?
        .into_iter()
        .take(budget.max_fuzzy_keys_per_term)
        .collect::<Vec<_>>();
    let mut candidate_ids = BTreeSet::new();

    cancellation.check()?;
    let mut selected_metadata = sources.metadata.postings_for_limit_checked(
        MetadataField::Comment,
        comment_terms,
        budget.max_metadata_ids_per_term,
        || cancellation.check(),
    )?;
    cancellation.check()?;
    selected_metadata.extend(sources.metadata.postings_for_limit_checked(
        MetadataField::Tag,
        tag_terms.clone(),
        budget.max_metadata_ids_per_term,
        || cancellation.check(),
    )?);
    let metadata = selected_metadata
        .into_iter()
        .filter_map(|posting| scope_metadata_posting(posting, scope))
        .map(|posting| {
            candidate_ids.extend(posting.ids.iter().copied());
            SearchMetadataPosting {
                field: match posting.field {
                    MetadataField::Tag => SearchMetadataField::Tag,
                    MetadataField::Comment => SearchMetadataField::Comment,
                },
                term: posting.term,
                ids: posting.ids,
            }
        })
        .collect::<Vec<_>>();

    cancellation.check()?;
    let substrings = scoped_substring_postings(
        sources.lookup,
        substring_grams,
        budget.max_substring_ids_per_gram,
        scope,
        cancellation,
    )?;
    for posting in &substrings {
        cancellation.check()?;
        candidate_ids.extend(posting.ids.iter().copied());
    }

    let mut prefix_candidates = prefix_terms.clone();
    let mut fuzzy_candidate_terms = BTreeSet::new();
    cancellation.check()?;
    let fuzzy = sources
        .lookup
        .fuzzy_postings_bounded_cancellable(
            fuzzy_keys,
            budget.max_fuzzy_terms_per_key,
            cancellation,
        )?
        .into_iter()
        .map(|posting| {
            let terms = posting
                .terms
                .into_iter()
                .filter(|term| {
                    fuzzy_candidate_terms.len() < budget.max_fuzzy_candidates_per_term
                        && fuzzy_candidate_terms.insert(term.clone())
                })
                .collect::<Vec<_>>();
            for term in &terms {
                prefix_candidates.push(term.clone());
            }
            SearchFuzzyPosting {
                key: posting.key,
                terms,
            }
        })
        .collect::<Vec<_>>();

    cancellation.check()?;
    let prefixes = scoped_prefix_postings(
        sources.lookup,
        prefix_candidates,
        budget.max_prefix_ids_per_term,
        scope,
        cancellation,
    )?;
    for posting in &prefixes {
        cancellation.check()?;
        candidate_ids.extend(posting.ids.iter().copied());
    }

    let content_postings = scope_content_postings(content.postings, scope);
    for posting in &content_postings {
        cancellation.check()?;
        candidate_ids.extend(posting.ids.iter().copied());
        candidate_ids.extend(posting.positions.iter().map(|positions| positions.id));
    }

    let has_positive_anchor = !content.terms.is_empty()
        || !tag_terms.is_empty()
        || !metadata.is_empty()
        || !prefixes.is_empty()
        || !substrings.is_empty()
        || !fuzzy.is_empty()
        || !content_postings.is_empty();
    let has_any_query = !parsed.terms.is_empty()
        || !parsed.excluded_terms.is_empty()
        || !parsed.phrases.is_empty()
        || !parsed.proximities.is_empty()
        || !parsed.filters.is_empty()
        || parsed.expression.is_some();
    let requires_full_record_hydration = !has_positive_anchor && has_any_query;

    Ok(SidecarQueryImport {
        report: SidecarQueryImportReport {
            metadata_postings: metadata.len(),
            prefix_postings: prefixes.len(),
            substring_postings: substrings.len(),
            fuzzy_postings: fuzzy.len(),
            content_postings: content_postings.len(),
            candidate_ids: candidate_ids.len(),
            requires_full_record_hydration,
        },
        metadata,
        prefixes,
        substrings,
        fuzzy,
        content: content_postings,
    })
}

fn scope_excludes_all(scope: &SearchVolumeScope) -> bool {
    matches!(scope, SearchVolumeScope::Only(volumes) if volumes.is_empty())
}

fn scope_metadata_posting(
    mut posting: MetadataPosting,
    scope: &SearchVolumeScope,
) -> Option<MetadataPosting> {
    posting.ids.retain(|id| scope.allows(id.volume));
    (!posting.ids.is_empty()).then_some(posting)
}

fn scope_content_postings(
    postings: Vec<ContentPosting>,
    scope: &SearchVolumeScope,
) -> Vec<ContentPosting> {
    postings
        .into_iter()
        .filter_map(|mut posting| {
            posting.ids.retain(|id| scope.allows(id.volume));
            posting
                .positions
                .retain(|positions| scope.allows(positions.id.volume));
            (!posting.ids.is_empty() || !posting.positions.is_empty()).then_some(posting)
        })
        .collect()
}

fn scoped_prefix_postings(
    lookup: &SearchArchiveLookup,
    prefixes: Vec<String>,
    limit: usize,
    scope: &SearchVolumeScope,
    cancellation: &Cancellation,
) -> Result<Vec<SearchPrefixPosting>> {
    match scope {
        SearchVolumeScope::All => {
            lookup.prefix_postings_bounded_cancellable(prefixes, limit, cancellation)
        }
        SearchVolumeScope::Only(volumes) if volumes.is_empty() || limit == 0 => Ok(Vec::new()),
        SearchVolumeScope::Only(volumes) => {
            let mut selected = BTreeSet::new();
            for prefix in prefixes {
                cancellation.check()?;
                if !prefix.is_empty() {
                    selected.insert(prefix);
                }
            }
            let mut postings = Vec::with_capacity(selected.len());
            for prefix in selected {
                cancellation.check()?;
                let mut ids = Vec::new();
                for volume in volumes {
                    cancellation.check()?;
                    ids.extend(
                        lookup
                            .prefix_ids_for_volume_bounded_cancellable(
                                &prefix,
                                *volume,
                                limit,
                                cancellation,
                            )?
                            .ids,
                    );
                }
                ids.sort();
                ids.dedup();
                ids.truncate(limit.saturating_mul(volumes.len()));
                postings.push(SearchPrefixPosting { prefix, ids });
            }
            Ok(postings)
        }
    }
}

fn scoped_substring_postings(
    lookup: &SearchArchiveLookup,
    grams: Vec<String>,
    limit: usize,
    scope: &SearchVolumeScope,
    cancellation: &Cancellation,
) -> Result<Vec<SearchSubstringPosting>> {
    match scope {
        SearchVolumeScope::All => Ok(lookup
            .substrings
            .postings_for_limit_checked(grams, limit, || cancellation.check())?
            .into_iter()
            .map(|posting| SearchSubstringPosting {
                gram: posting.gram,
                ids: posting.ids,
            })
            .collect()),
        SearchVolumeScope::Only(volumes) if volumes.is_empty() || limit == 0 => Ok(Vec::new()),
        SearchVolumeScope::Only(volumes) => {
            let mut selected = BTreeSet::new();
            for gram in grams {
                cancellation.check()?;
                if !gram.is_empty() {
                    selected.insert(gram);
                }
            }
            let mut postings = Vec::with_capacity(selected.len());
            for gram in selected {
                cancellation.check()?;
                let mut ids = Vec::new();
                for volume in volumes {
                    cancellation.check()?;
                    ids.extend(
                        lookup
                            .substring_ids_for_volume_bounded_cancellable(
                                &gram,
                                *volume,
                                limit,
                                cancellation,
                            )?
                            .ids,
                    );
                }
                ids.sort();
                ids.dedup();
                ids.truncate(limit.saturating_mul(volumes.len()));
                postings.push(SearchSubstringPosting { gram, ids });
            }
            Ok(postings)
        }
    }
}

pub(crate) fn sidecar_candidate_ids_cancellable(
    import: &SidecarQueryImport,
    cancellation: &Cancellation,
) -> Result<BTreeSet<FileId>> {
    import.candidate_ids_cancellable(cancellation)
}

fn canonical_content_query_term_checked(
    term: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<String> {
    check_control()?;
    let mut canonical = String::new();
    for (index, ch) in term.trim().chars().enumerate() {
        if index.is_multiple_of(SIDECAR_CONTENT_TERM_CHECK_STRIDE) {
            check_control()?;
        }
        canonical.extend(ch.to_lowercase());
    }
    check_control()?;
    Ok(canonical)
}

fn bounded_substring_grams(terms: &[String], budget: SearchLookupBudget) -> Vec<String> {
    let mut grams = BTreeSet::new();
    for term in terms {
        grams.extend(
            substring_candidate_grams(term)
                .into_iter()
                .take(budget.max_substring_grams_per_term),
        );
    }
    grams.into_iter().collect()
}

fn empty_sidecar_query_session_report() -> SidecarQuerySessionReport {
    SidecarQuerySessionReport {
        hydration: SidecarRecordHydrationReport::default(),
        search: SearchQueryReport {
            hits: Vec::new(),
            lookup: SearchLookupTelemetry::default(),
        },
        content_cache_hits: 0,
        content_cache_misses: 0,
        record_cache_hits: 0,
        record_cache_misses: 0,
        result_cache_hits: 0,
        result_cache_misses: 0,
    }
}

fn bounded_posting_cache_key(term: &str, limit: usize) -> String {
    format!("{limit}:{term}")
}

fn bounded_volume_posting_cache_key(term: &str, volume: VolumeId, limit: usize) -> String {
    format!("{limit}:volume={}:{}", volume.0, term)
}

fn query_result_cache_key(
    query: &SearchQuery,
    limit: usize,
    scope: &SearchVolumeScope,
    budget: SearchLookupBudget,
) -> String {
    format!(
        "{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}",
        query.canonical_cache_key(),
        limit,
        search_volume_scope_cache_key(scope),
        budget.max_prefix_ids_per_term,
        budget.min_archive_prefix_chars,
        budget.max_substring_grams_per_term,
        budget.max_substring_ids_per_gram,
        budget.max_fuzzy_keys_per_term,
        budget.max_fuzzy_terms_per_key,
        budget.max_fuzzy_candidates_per_term,
        budget.max_metadata_ids_per_term,
        budget.max_content_ids_per_term
    )
}

fn search_volume_scope_cache_key(scope: &SearchVolumeScope) -> String {
    match scope {
        SearchVolumeScope::All => "all".to_string(),
        SearchVolumeScope::Only(volumes) => {
            let mut key = String::from("only:");
            for volume in volumes {
                key.push_str(&volume.0.to_string());
                key.push(',');
            }
            key
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_jobs::Cancellation;
    use gfm_search::SearchLookup;
    use gfm_store::{
        fuzzy_postings_from_records, metadata_postings_from_records, prefix_postings_from_records,
        substring_postings_from_records, write_content_postings, write_fuzzy_postings,
        write_metadata_postings, write_prefix_postings, write_record_columns, write_records,
        write_substring_postings,
    };
    use gfm_types::{ContentPositions, FileKind, GfmError, VolumeId};
    use std::fs;
    use std::panic::{self, AssertUnwindSafe};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn sidecar_session_recovers_poisoned_content_cache() {
        let fixture = SidecarFixture::new("content-cache");
        let session = fixture.session();

        poison_sidecar_content_cache(&session);
        let report = session.search("finderlatency", 5).unwrap();

        assert_eq!(report.search.hits.len(), 1);
        assert_eq!(report.search.hits[0].record.id, fixture.record.id);
        assert_eq!(report.content_cache_misses, 1);
        assert_eq!(report.record_cache_misses, 1);
    }

    #[test]
    fn sidecar_session_recovers_poisoned_record_cache() {
        let fixture = SidecarFixture::new("record-cache");
        let session = fixture.session();
        let first = session.search("finderlatency", 5).unwrap();
        assert_eq!(first.search.hits.len(), 1);

        poison_sidecar_record_cache(&session);
        let second = session.search("finderlatency", 6).unwrap();

        assert_eq!(second.search.hits.len(), 1);
        assert_eq!(second.search.hits[0].record.id, fixture.record.id);
        assert_eq!(second.content_cache_hits, 1);
        assert_eq!(second.record_cache_hits, 1);
    }

    #[test]
    fn sidecar_session_reuses_exact_query_results() {
        let fixture = SidecarFixture::new("result-cache");
        let session = fixture.session();

        let first = session.search("finderlatency", 5).unwrap();
        let second = session.search("finderlatency", 5).unwrap();

        assert_eq!(first.search.hits, second.search.hits);
        assert_eq!(first.result_cache_hits, 0);
        assert_eq!(first.result_cache_misses, 1);
        assert_eq!(second.result_cache_hits, 1);
        assert_eq!(second.result_cache_misses, 0);
        assert_eq!(session.result_cache_telemetry(), (1, 1));
    }

    #[test]
    fn sidecar_session_reuses_normalized_query_results() {
        let fixture = SidecarFixture::new("normalized-result-cache");
        let session = fixture.session();

        let first = session.search("  FinderLatency  ", 5).unwrap();
        let second = session.search("finderlatency", 5).unwrap();

        assert_eq!(first.search.hits, second.search.hits);
        assert_eq!(first.result_cache_hits, 0);
        assert_eq!(first.result_cache_misses, 1);
        assert_eq!(second.result_cache_hits, 1);
        assert_eq!(second.result_cache_misses, 0);
        assert_eq!(second.content_cache_hits, 0);
        assert_eq!(second.content_cache_misses, 0);
        assert_eq!(second.record_cache_hits, 0);
        assert_eq!(second.record_cache_misses, 0);
        assert_eq!(session.result_cache_telemetry(), (1, 1));
    }

    #[test]
    fn provider_metadata_invalidation_clears_sidecar_query_results() {
        let fixture = SidecarFixture::new("provider-result-cache-clear");
        let session = fixture.session();
        let first = session.search("finderlatency", 5).unwrap();
        let cached = session.search("finderlatency", 5).unwrap();
        assert_eq!(first.search.hits, cached.search.hits);
        assert_eq!(cached.result_cache_hits, 1);

        let provider = crate::ProviderMetadataInvalidationReport::from_provider_transition(
            fixture.record.path.clone(),
            "downloaded",
            "evicted",
            true,
            true,
            "fileprovider-state-changed",
        );
        let invalidation = session.apply_provider_metadata_invalidation(&provider);
        let after = session.search("finderlatency", 5).unwrap();

        assert!(invalidation.invalidated);
        assert_eq!(invalidation.result_entries_before, 1);
        assert_eq!(invalidation.result_entries_after, 0);
        assert_eq!(invalidation.reason, "provider-metadata-state-changed");
        assert_eq!(
            invalidation.as_tsv(),
            "sidecar-query-cache-invalidation\t/tmp/FinderLatency.md\tinvalidated=true\tresult-entries-before=1\tresult-entries-after=0\treason=provider-metadata-state-changed"
        );
        assert_eq!(after.result_cache_hits, 0);
        assert_eq!(after.result_cache_misses, 1);
        assert_eq!(after.content_cache_hits, 1);
    }

    #[test]
    fn sidecar_query_cache_invalidation_tsv_escapes_control_characters() {
        let report = SidecarQueryCacheInvalidationReport {
            path: std::path::PathBuf::from("/tmp/Sidecar\\Rows/Query\tDraft\nFinal\r.md"),
            invalidated: true,
            result_entries_before: 3,
            result_entries_after: 0,
            reason: "provider\tchanged\nagain\r".to_string(),
        };
        let tsv = report.as_tsv();

        assert_eq!(tsv.lines().count(), 1, "{tsv}");
        assert!(!tsv.contains('\r'), "{tsv}");
        assert!(
            tsv.contains("Sidecar\\\\Rows/Query\\tDraft\\nFinal\\r.md\t"),
            "{tsv}"
        );
        assert!(
            tsv.contains("reason=provider\\tchanged\\nagain\\r"),
            "{tsv}"
        );
        assert_eq!(tsv.split('\t').count(), 6, "{tsv}");
    }

    #[test]
    fn provider_metadata_noop_preserves_sidecar_query_results() {
        let fixture = SidecarFixture::new("provider-result-cache-noop");
        let session = fixture.session();
        let first = session.search("finderlatency", 5).unwrap();
        let cached = session.search("finderlatency", 5).unwrap();
        assert_eq!(first.search.hits, cached.search.hits);
        assert_eq!(cached.result_cache_hits, 1);

        let provider = crate::ProviderMetadataInvalidationReport::from_provider_transition(
            fixture.record.path.clone(),
            "downloaded",
            "downloaded",
            true,
            false,
            "fileprovider-state-unchanged",
        );
        let invalidation = session.apply_provider_metadata_invalidation(&provider);
        let after = session.search("finderlatency", 5).unwrap();

        assert!(!invalidation.invalidated);
        assert_eq!(invalidation.result_entries_before, 1);
        assert_eq!(invalidation.result_entries_after, 1);
        assert_eq!(invalidation.reason, "provider-state-unchanged");
        assert_eq!(after.result_cache_hits, 1);
        assert_eq!(after.result_cache_misses, 0);
    }

    #[test]
    fn lookup_cache_refreshes_recency_on_hit() {
        let mut cache = LookupCache::new(2);
        cache.insert("first".to_string(), vec![1]);
        cache.insert("second".to_string(), vec![2]);

        assert_eq!(cache.get("first"), Some(vec![1]));
        cache.insert("third".to_string(), vec![3]);

        assert_eq!(cache.get("first"), Some(vec![1]));
        assert_eq!(cache.get("second"), None);
    }

    #[test]
    fn lookup_cache_refreshes_recency_on_update() {
        let mut cache = LookupCache::new(2);
        cache.insert("first".to_string(), vec![1]);
        cache.insert("second".to_string(), vec![2]);
        cache.insert("first".to_string(), vec![11]);
        cache.insert("third".to_string(), vec![3]);

        assert_eq!(cache.get("first"), Some(vec![11]));
        assert_eq!(cache.get("second"), None);
    }

    #[test]
    fn sidecar_session_empty_zero_limit_and_empty_scope_skip_cache_work() {
        let fixture = SidecarFixture::new("empty-query");
        let session = fixture.session();

        let empty = session.search("   ", 5).unwrap();
        let zero_limit = session.search("finderlatency", 0).unwrap();
        let empty_scope = session
            .search_with_volume_scope("finderlatency", 5, &SearchVolumeScope::only([]))
            .unwrap();

        assert!(empty.search.hits.is_empty());
        assert_eq!(empty.hydration, SidecarRecordHydrationReport::default());
        assert_eq!(zero_limit, empty);
        assert_eq!(empty_scope, empty);
        assert_eq!(session.content_cache_telemetry(), (0, 0));
        assert_eq!(session.record_cache_telemetry(), (0, 0));
        assert_eq!(session.result_cache_telemetry(), (0, 0));
        assert_eq!(
            session.lookup.cache_telemetry(),
            SearchLookupTelemetry::default()
        );
    }

    #[test]
    fn sidecar_session_honors_pre_cancelled_queries_without_cache_work() {
        let fixture = SidecarFixture::new("pre-cancelled");
        let session = fixture.session();
        let cancellation = Cancellation::default();
        cancellation.cancel();

        let result = session.search_cancellable("finderlatency", 5, &cancellation);

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert_eq!(session.content_cache_telemetry(), (0, 0));
        assert_eq!(session.record_cache_telemetry(), (0, 0));
    }

    #[test]
    fn sidecar_candidate_expansion_honors_cancelled_tokens() {
        let cancellation = Cancellation::default();
        cancellation.cancel();
        let import = SidecarQueryImport {
            metadata: vec![SearchMetadataPosting {
                field: SearchMetadataField::Tag,
                term: "needle".to_string(),
                ids: vec![FileId::new(VolumeId(1), 1)],
            }],
            prefixes: vec![SearchPrefixPosting {
                prefix: "nee".to_string(),
                ids: vec![FileId::new(VolumeId(1), 2)],
            }],
            substrings: Vec::new(),
            fuzzy: Vec::new(),
            content: vec![ContentPosting {
                term: "needle".to_string(),
                ids: vec![FileId::new(VolumeId(1), 3)],
                positions: vec![ContentPositions {
                    id: FileId::new(VolumeId(1), 4),
                    positions: vec![0],
                }],
            }],
            report: SidecarQueryImportReport::default(),
        };

        let result = sidecar_candidate_ids_cancellable(&import, &cancellation);

        assert!(matches!(result, Err(GfmError::Cancelled)));
    }

    #[test]
    fn sidecar_content_query_term_canonicalization_honors_checked_control() {
        let mut checks = 0usize;
        let result = canonical_content_query_term_checked(&"FinderLatency".repeat(256), || {
            checks += 1;
            if checks >= 3 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(checks >= 3);
    }

    #[test]
    fn query_sidecar_import_honors_pre_cancelled_queries_without_lookup_work() {
        let fixture = SidecarFixture::new("pre-cancelled-import");
        let session = fixture.session();
        let cancellation = Cancellation::default();
        cancellation.cancel();

        let result = query_sidecar_imports_cancellable(
            &session.metadata,
            &session.lookup,
            &session.lookup.substrings,
            &session.content,
            "finderlatency",
            SearchLookupBudget::default(),
            &cancellation,
        );

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert_eq!(
            session.lookup.cache_telemetry(),
            SearchLookupTelemetry::default()
        );
        assert_eq!(session.lookup.cache_entry_counts().unwrap(), (0, 0, 0));
    }

    #[test]
    fn search_archive_lookup_recovers_poisoned_prefix_cache() {
        let fixture = SidecarFixture::new("prefix-cache");
        let lookup = fixture.lookup();

        poison_prefix_cache(&lookup);
        let ids = lookup.prefix_ids("finder").unwrap();

        assert_eq!(ids, vec![fixture.record.id]);
    }

    #[test]
    fn search_archive_lookup_recovers_poisoned_substring_cache() {
        let fixture = SidecarFixture::new("substring-cache");
        let lookup = fixture.lookup();

        poison_substring_cache(&lookup);
        let ids = lookup.substring_ids("lat").unwrap();

        assert_eq!(ids, vec![fixture.record.id]);
    }

    #[test]
    fn search_archive_lookup_recovers_poisoned_fuzzy_cache() {
        let fixture = SidecarFixture::new("fuzzy-cache");
        let lookup = fixture.lookup();

        poison_fuzzy_cache(&lookup);
        let terms = lookup.fuzzy_terms("finderlatency").unwrap();

        assert!(terms.contains(&"finderlatency".to_string()));
    }

    #[test]
    fn sidecar_session_searches_and_hydrates_only_admitted_volume_scope() {
        let primary = record(FileId::new(VolumeId(7), 42));
        let mut secondary = record(FileId::new(VolumeId(8), 43));
        secondary.path = PathBuf::from("/Volumes/Fast/FinderLatency.md");
        secondary.tags = vec!["Fast".to_string()];
        secondary.finder_comment = Some("instant search".to_string());
        let fixture = SidecarFixture::from_records("scoped-volume", vec![primary, secondary]);
        let session = fixture.session();

        let report = session
            .search_with_volume_scope("finderlatency", 10, &SearchVolumeScope::only([VolumeId(8)]))
            .unwrap();

        assert_eq!(report.search.hits.len(), 1);
        assert_eq!(report.search.hits[0].record.id.volume, VolumeId(8));
        assert_eq!(report.hydration.records_loaded, 1);
        assert_eq!(report.hydration.import.candidate_ids, 1);
        assert!(report.hydration.import.prefix_postings > 0);
        assert!(report.hydration.import.substring_postings > 0);
        assert_eq!(report.hydration.import.content_postings, 1);
        assert_eq!(report.content_cache_misses, 1);

        let cached = session
            .search_with_volume_scope("finderlatency", 10, &SearchVolumeScope::only([VolumeId(8)]))
            .unwrap();

        assert_eq!(cached.search.hits.len(), 1);
        assert_eq!(cached.result_cache_hits, 1);
        assert_eq!(cached.result_cache_misses, 0);
    }

    #[test]
    fn sidecar_session_empty_volume_scope_hydrates_no_records_or_sidecars() {
        let fixture = SidecarFixture::new("empty-scoped-volume");
        let session = fixture.session();

        let report = session
            .search_with_volume_scope("finderlatency", 10, &SearchVolumeScope::only([]))
            .unwrap();

        assert!(report.search.hits.is_empty());
        assert_eq!(report.hydration.records_loaded, 0);
        assert_eq!(report.hydration.import, SidecarQueryImportReport::default());
        assert_eq!(report.content_cache_misses, 0);
        assert_eq!(report.record_cache_misses, 0);
        assert_eq!(
            session.lookup.cache_telemetry(),
            SearchLookupTelemetry::default()
        );
    }

    fn poison_sidecar_content_cache(session: &SidecarIndexQuerySession) {
        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            let _guard = session
                .content_cache
                .lock()
                .expect("initial sidecar content cache lock");
            panic!("poison sidecar content cache");
        }));
    }

    fn poison_sidecar_record_cache(session: &SidecarIndexQuerySession) {
        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            let _guard = session
                .record_cache
                .lock()
                .expect("initial sidecar record cache lock");
            panic!("poison sidecar record cache");
        }));
    }

    fn poison_prefix_cache(lookup: &SearchArchiveLookup) {
        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            let _guard = lookup
                .prefix_cache
                .lock()
                .expect("initial prefix cache lock");
            panic!("poison prefix cache");
        }));
    }

    fn poison_substring_cache(lookup: &SearchArchiveLookup) {
        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            let _guard = lookup
                .substring_cache
                .lock()
                .expect("initial substring cache lock");
            panic!("poison substring cache");
        }));
    }

    fn poison_fuzzy_cache(lookup: &SearchArchiveLookup) {
        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            let _guard = lookup.fuzzy_cache.lock().expect("initial fuzzy cache lock");
            panic!("poison fuzzy cache");
        }));
    }

    struct SidecarFixture {
        root: PathBuf,
        records: PathBuf,
        columns: PathBuf,
        metadata: PathBuf,
        prefixes: PathBuf,
        substrings: PathBuf,
        fuzzy: PathBuf,
        content: PathBuf,
        record: FileRecord,
    }

    impl SidecarFixture {
        fn new(name: &str) -> Self {
            Self::from_records(name, vec![record(FileId::new(VolumeId(7), 42))])
        }

        fn from_records(name: &str, record_set: Vec<FileRecord>) -> Self {
            let root = temp_dir(&format!("gfm-sidecar-poison-{name}"));
            let records = root.join("records.gfmidx");
            let columns = root.join("columns.gfmcols");
            let metadata = root.join("metadata.gfmmeta");
            let prefixes = root.join("prefixes.gfmprefix");
            let substrings = root.join("substrings.gfmsubstr");
            let fuzzy = root.join("fuzzy.gfmfuzzy");
            let content = root.join("content.gfmcontent");
            let record = record_set.first().expect("sidecar fixture record").clone();

            write_records(&records, &record_set).unwrap();
            write_record_columns(&columns, &record_set).unwrap();
            write_metadata_postings(&metadata, &metadata_postings_from_records(&record_set))
                .unwrap();
            write_prefix_postings(&prefixes, &prefix_postings_from_records(&record_set)).unwrap();
            write_substring_postings(&substrings, &substring_postings_from_records(&record_set))
                .unwrap();
            write_fuzzy_postings(&fuzzy, &fuzzy_postings_from_records(&record_set)).unwrap();
            write_content_postings(
                &content,
                &[content_posting_from_records("finderlatency", &record_set)],
            )
            .unwrap();

            Self {
                root,
                records,
                columns,
                metadata,
                prefixes,
                substrings,
                fuzzy,
                content,
                record,
            }
        }

        fn session(&self) -> SidecarIndexQuerySession {
            SidecarIndexQuerySession::open(
                &self.records,
                &self.columns,
                &self.metadata,
                &self.prefixes,
                &self.substrings,
                &self.fuzzy,
                &self.content,
            )
            .unwrap()
        }

        fn lookup(&self) -> SearchArchiveLookup {
            SearchArchiveLookup::open(&self.prefixes, &self.substrings, &self.fuzzy).unwrap()
        }
    }

    impl Drop for SidecarFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn record(id: FileId) -> FileRecord {
        FileRecord {
            id,
            parent: None,
            path: PathBuf::from("/tmp/FinderLatency.md"),
            name: "FinderLatency.md".to_string(),
            kind: FileKind::File,
            len: 12,
            mode: 0o100644,
            owner: 501,
            group: 20,
            xattrs_digest: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: vec!["Important".to_string()],
            finder_comment: Some("instant search".to_string()),
        }
    }

    fn content_posting_from_records(term: &str, records: &[FileRecord]) -> ContentPosting {
        ContentPosting {
            term: term.to_string(),
            ids: records.iter().map(|record| record.id).collect(),
            positions: records
                .iter()
                .map(|record| ContentPositions {
                    id: record.id,
                    positions: vec![1],
                })
                .collect(),
        }
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }
}

#[derive(Debug)]
struct LookupCache<V> {
    capacity: usize,
    order: VecDeque<String>,
    values: HashMap<String, V>,
}

#[derive(Debug)]
struct RecordCache {
    capacity: usize,
    order: VecDeque<FileId>,
    values: HashMap<FileId, HydratedRecord>,
}

impl RecordCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            order: VecDeque::with_capacity(capacity),
            values: HashMap::new(),
        }
    }

    fn get(&self, id: FileId) -> Option<HydratedRecord> {
        self.values.get(&id).cloned()
    }

    fn insert(&mut self, id: FileId, record: HydratedRecord) {
        if self.capacity == 0 {
            return;
        }
        if !self.values.contains_key(&id) {
            self.order.push_back(id);
        }
        self.values.insert(id, record);
        while self.values.len() > self.capacity {
            let Some(expired) = self.order.pop_front() else {
                break;
            };
            self.values.remove(&expired);
        }
    }
}

impl<V: Clone> LookupCache<V> {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            order: VecDeque::with_capacity(capacity),
            values: HashMap::new(),
        }
    }

    fn get(&mut self, key: &str) -> Option<V> {
        let value = self.values.get(key).cloned()?;
        refresh_string_recency(&mut self.order, key);
        Some(value)
    }

    fn insert(&mut self, key: String, value: V) {
        if self.capacity == 0 {
            return;
        }
        if self.values.contains_key(&key) {
            refresh_string_recency(&mut self.order, &key);
        } else {
            self.order.push_back(key.clone());
        }
        self.values.insert(key, value);
        while self.values.len() > self.capacity {
            let Some(expired) = self.order.pop_front() else {
                break;
            };
            self.values.remove(&expired);
        }
    }

    fn clear(&mut self) {
        self.order.clear();
        self.values.clear();
    }

    fn len(&self) -> usize {
        self.values.len()
    }
}

fn refresh_string_recency(order: &mut VecDeque<String>, key: &str) {
    let Some(index) = order.iter().position(|candidate| candidate == key) else {
        return;
    };
    let Some(key) = order.remove(index) else {
        return;
    };
    order.push_back(key);
}
