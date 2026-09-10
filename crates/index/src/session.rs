use crate::lookup::{SidecarIndexQuerySession, SidecarQuerySessionReport};
use crate::{ContentQueryLoadReport, LiveIndex, ProviderMetadataInvalidationReport};
use gfm_jobs::Cancellation;
use gfm_search::{
    SearchLookupBudget, SearchLookupTelemetry, SearchQuery, SearchQueryReport, SearchVolumeScope,
};
use gfm_store::{LimitedContentPosting, MmapContentSet, MmapRecordArchive};
use gfm_types::{ContentPosting, FileId, FileRecord, Result, VolumeId};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

const CONTENT_RECORD_CACHE_CAPACITY: usize = 8192;
const CONTENT_POSTING_CACHE_CAPACITY: usize = 512;
const CONTENT_QUERY_RESULT_CACHE_CAPACITY: usize = 256;
const CONTENT_QUERY_TERM_CHECK_STRIDE: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentQuerySessionReport {
    pub load: ContentQueryLoadReport,
    pub search: SearchQueryReport,
    pub posting_cache_hits: usize,
    pub posting_cache_misses: usize,
    pub record_cache_hits: usize,
    pub record_cache_misses: usize,
    pub result_cache_hits: usize,
    pub result_cache_misses: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentQueryCacheInvalidationReport {
    pub path: std::path::PathBuf,
    pub invalidated: bool,
    pub result_entries_before: usize,
    pub result_entries_after: usize,
    pub reason: String,
}

impl ContentQueryCacheInvalidationReport {
    pub fn as_tsv(&self) -> String {
        format!(
            "content-query-cache-invalidation\t{}\tinvalidated={}\tresult-entries-before={}\tresult-entries-after={}\treason={}",
            escape_tsv_field(&self.path.to_string_lossy()),
            self.invalidated,
            self.result_entries_before,
            self.result_entries_after,
            escape_tsv_field(&self.reason)
        )
    }
}

#[derive(Debug, Default)]
pub struct IndexQuerySupersession {
    active: Mutex<Option<Cancellation>>,
}

impl IndexQuerySupersession {
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

    pub fn search_sidecar(
        &self,
        session: &SidecarIndexQuerySession,
        query: &str,
        limit: usize,
    ) -> Result<SidecarQuerySessionReport> {
        let cancellation = self.begin();
        session.search_cancellable(query, limit, &cancellation)
    }

    pub fn search_sidecar_with_budget(
        &self,
        session: &SidecarIndexQuerySession,
        query: &str,
        limit: usize,
        budget: SearchLookupBudget,
    ) -> Result<SidecarQuerySessionReport> {
        let cancellation = self.begin();
        session.search_with_budget_cancellable(query, limit, budget, &cancellation)
    }

    pub fn search_sidecar_with_volume_scope(
        &self,
        session: &SidecarIndexQuerySession,
        query: &str,
        limit: usize,
        scope: &SearchVolumeScope,
    ) -> Result<SidecarQuerySessionReport> {
        let cancellation = self.begin();
        session.search_with_volume_scope_budget_cancellable(
            query,
            limit,
            scope,
            SearchLookupBudget::default(),
            &cancellation,
        )
    }

    pub fn search_content(
        &self,
        session: &ContentIndexQuerySession,
        query: &str,
        limit: usize,
    ) -> Result<ContentQuerySessionReport> {
        let cancellation = self.begin();
        session.search_cancellable(query, limit, &cancellation)
    }

    pub fn search_content_with_budget(
        &self,
        session: &ContentIndexQuerySession,
        query: &str,
        limit: usize,
        budget: SearchLookupBudget,
    ) -> Result<ContentQuerySessionReport> {
        let cancellation = self.begin();
        session.search_with_budget_cancellable(query, limit, budget, &cancellation)
    }

    pub fn search_content_with_volume_scope(
        &self,
        session: &ContentIndexQuerySession,
        query: &str,
        limit: usize,
        scope: &SearchVolumeScope,
    ) -> Result<ContentQuerySessionReport> {
        let cancellation = self.begin();
        session.search_with_volume_scope_budget_cancellable(
            query,
            limit,
            scope,
            SearchLookupBudget::default(),
            &cancellation,
        )
    }

    fn active_lock(&self) -> MutexGuard<'_, Option<Cancellation>> {
        self.active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Debug)]
pub struct ContentIndexQuerySession {
    records: MmapRecordArchive,
    content: MmapContentSet,
    posting_cache: Mutex<ContentPostingCache>,
    posting_cache_hits: AtomicUsize,
    posting_cache_misses: AtomicUsize,
    record_cache: Mutex<ContentRecordCache>,
    record_cache_hits: AtomicUsize,
    record_cache_misses: AtomicUsize,
    result_cache: Mutex<ContentResultCache>,
    result_cache_hits: AtomicUsize,
    result_cache_misses: AtomicUsize,
}

impl ContentIndexQuerySession {
    pub fn open_content(
        records_path: impl AsRef<Path>,
        content_path: impl AsRef<Path>,
    ) -> Result<Self> {
        Self::open_content_cancellable(records_path, content_path, &Cancellation::default())
    }

    pub fn open_content_cancellable(
        records_path: impl AsRef<Path>,
        content_path: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<Self> {
        Self::open_set_cancellable(records_path, std::iter::once(content_path), cancellation)
    }

    pub fn open_set<I, P>(records_path: impl AsRef<Path>, content_paths: I) -> Result<Self>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        Self::open_set_cancellable(records_path, content_paths, &Cancellation::default())
    }

    pub fn open_set_cancellable<I, P>(
        records_path: impl AsRef<Path>,
        content_paths: I,
        cancellation: &Cancellation,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        cancellation.check()?;
        let records = MmapRecordArchive::open_checked(records_path, || cancellation.check())?;
        cancellation.check()?;
        let content = MmapContentSet::open_checked(content_paths, || cancellation.check())?;
        cancellation.check()?;
        Ok(Self {
            records,
            content,
            posting_cache: Mutex::new(ContentPostingCache::new(CONTENT_POSTING_CACHE_CAPACITY)),
            posting_cache_hits: AtomicUsize::new(0),
            posting_cache_misses: AtomicUsize::new(0),
            record_cache: Mutex::new(ContentRecordCache::new(CONTENT_RECORD_CACHE_CAPACITY)),
            record_cache_hits: AtomicUsize::new(0),
            record_cache_misses: AtomicUsize::new(0),
            result_cache: Mutex::new(ContentResultCache::new(CONTENT_QUERY_RESULT_CACHE_CAPACITY)),
            result_cache_hits: AtomicUsize::new(0),
            result_cache_misses: AtomicUsize::new(0),
        })
    }

    pub fn open_manifest(
        records_path: impl AsRef<Path>,
        manifest_path: impl AsRef<Path>,
    ) -> Result<Self> {
        Self::open_manifest_cancellable(records_path, manifest_path, &Cancellation::default())
    }

    pub fn open_manifest_cancellable(
        records_path: impl AsRef<Path>,
        manifest_path: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<Self> {
        cancellation.check()?;
        let records = MmapRecordArchive::open_checked(records_path, || cancellation.check())?;
        cancellation.check()?;
        let content =
            MmapContentSet::open_manifest_checked(manifest_path, || cancellation.check())?;
        cancellation.check()?;
        Ok(Self {
            records,
            content,
            posting_cache: Mutex::new(ContentPostingCache::new(CONTENT_POSTING_CACHE_CAPACITY)),
            posting_cache_hits: AtomicUsize::new(0),
            posting_cache_misses: AtomicUsize::new(0),
            record_cache: Mutex::new(ContentRecordCache::new(CONTENT_RECORD_CACHE_CAPACITY)),
            record_cache_hits: AtomicUsize::new(0),
            record_cache_misses: AtomicUsize::new(0),
            result_cache: Mutex::new(ContentResultCache::new(CONTENT_QUERY_RESULT_CACHE_CAPACITY)),
            result_cache_hits: AtomicUsize::new(0),
            result_cache_misses: AtomicUsize::new(0),
        })
    }

    pub fn indexed_records(&self) -> usize {
        self.records.len()
    }

    pub fn archive_count(&self) -> usize {
        self.content.archive_count()
    }

    pub fn posting_cache_telemetry(&self) -> (usize, usize) {
        (
            self.posting_cache_hits.load(Ordering::Relaxed),
            self.posting_cache_misses.load(Ordering::Relaxed),
        )
    }

    pub fn record_cache_telemetry(&self) -> (usize, usize) {
        (
            self.record_cache_hits.load(Ordering::Relaxed),
            self.record_cache_misses.load(Ordering::Relaxed),
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
        report: &ProviderMetadataInvalidationReport,
    ) -> ContentQueryCacheInvalidationReport {
        let mut cache = self.result_cache_lock();
        let result_entries_before = cache.len();
        if report.invalidate_query_cache {
            cache.clear();
        }
        let result_entries_after = cache.len();
        ContentQueryCacheInvalidationReport {
            path: report.path.clone(),
            invalidated: report.invalidate_query_cache,
            result_entries_before,
            result_entries_after,
            reason: report.reason.clone(),
        }
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<ContentQuerySessionReport> {
        self.search_with_budget(query, limit, SearchLookupBudget::default())
    }

    pub fn search_with_budget(
        &self,
        query: &str,
        limit: usize,
        budget: SearchLookupBudget,
    ) -> Result<ContentQuerySessionReport> {
        self.search_structured_with_budget_cancellable(
            &SearchQuery::parse(query),
            limit,
            budget,
            &Cancellation::default(),
        )
    }

    pub fn search_cancellable(
        &self,
        query: &str,
        limit: usize,
        cancellation: &Cancellation,
    ) -> Result<ContentQuerySessionReport> {
        self.search_with_budget_cancellable(
            query,
            limit,
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
    ) -> Result<ContentQuerySessionReport> {
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
    ) -> Result<ContentQuerySessionReport> {
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
    ) -> Result<ContentQuerySessionReport> {
        let query = SearchQuery::parse_cancellable(query, cancellation)?;
        self.search_structured_with_volume_scope_budget_cancellable(
            &query,
            limit,
            scope,
            budget,
            cancellation,
        )
    }

    pub fn search_structured_with_budget_cancellable(
        &self,
        parsed: &SearchQuery,
        limit: usize,
        budget: SearchLookupBudget,
        cancellation: &Cancellation,
    ) -> Result<ContentQuerySessionReport> {
        self.search_structured_with_volume_scope_budget_cancellable(
            parsed,
            limit,
            &SearchVolumeScope::All,
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
    ) -> Result<ContentQuerySessionReport> {
        cancellation.check()?;
        if parsed.is_empty()
            || limit == 0
            || scope_excludes_all(scope)
            || !self.records_contains_scope(scope)
        {
            return Ok(empty_content_query_session_report());
        }
        let result_cache_key = content_query_result_cache_key(parsed, limit, scope, budget);
        if let Some(mut report) = self.result_cache_lock().get(&result_cache_key) {
            self.result_cache_hits.fetch_add(1, Ordering::Relaxed);
            report.search.lookup = SearchLookupTelemetry::default();
            report.posting_cache_hits = 0;
            report.posting_cache_misses = 0;
            report.record_cache_hits = 0;
            report.record_cache_misses = 0;
            report.result_cache_hits = 1;
            report.result_cache_misses = 0;
            return Ok(report);
        }
        self.result_cache_misses.fetch_add(1, Ordering::Relaxed);
        let posting_hits_before = self.posting_cache_hits.load(Ordering::Relaxed);
        let posting_misses_before = self.posting_cache_misses.load(Ordering::Relaxed);
        let content_terms = parsed.content_candidate_terms_cancellable(cancellation)?;
        let has_content_terms = !content_terms.is_empty();
        let postings =
            self.scoped_postings_for_terms(content_terms, scope, budget, cancellation)?;
        cancellation.check()?;
        let cache_hits_before = self.record_cache_hits.load(Ordering::Relaxed);
        let cache_misses_before = self.record_cache_misses.load(Ordering::Relaxed);
        let (live, load) =
            self.live_from_postings(postings, has_content_terms, scope, cancellation)?;
        let hits = live.search_structured_with_volume_scope_cancellable(
            parsed,
            limit,
            scope,
            cancellation,
        )?;
        let report = ContentQuerySessionReport {
            load,
            search: SearchQueryReport {
                hits,
                lookup: SearchLookupTelemetry::default(),
            },
            posting_cache_hits: self
                .posting_cache_hits
                .load(Ordering::Relaxed)
                .saturating_sub(posting_hits_before),
            posting_cache_misses: self
                .posting_cache_misses
                .load(Ordering::Relaxed)
                .saturating_sub(posting_misses_before),
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

    fn scoped_postings_for_terms(
        &self,
        terms: Vec<String>,
        scope: &SearchVolumeScope,
        budget: SearchLookupBudget,
        cancellation: &Cancellation,
    ) -> Result<Vec<ContentPosting>> {
        match scope {
            SearchVolumeScope::All => self.postings_for_terms(terms, budget, cancellation),
            SearchVolumeScope::Only(volumes) => {
                let mut postings = Vec::new();
                for volume in volumes {
                    cancellation.check()?;
                    if self.records.contains_volume(*volume) {
                        postings.extend(self.postings_for_terms_in_volume(
                            terms.clone(),
                            *volume,
                            budget,
                            cancellation,
                        )?);
                    }
                }
                postings.sort_by(|left, right| {
                    left.term
                        .cmp(&right.term)
                        .then_with(|| left.ids.first().cmp(&right.ids.first()))
                });
                Ok(postings)
            }
        }
    }

    fn postings_for_terms(
        &self,
        terms: Vec<String>,
        budget: SearchLookupBudget,
        cancellation: &Cancellation,
    ) -> Result<Vec<ContentPosting>> {
        let mut selected = BTreeSet::new();
        for term in terms {
            cancellation.check()?;
            let term = canonical_query_term_checked(&term, || cancellation.check())?;
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
            let mut cache = self.posting_cache_lock();
            for term in &selected {
                cancellation.check()?;
                let key = posting_cache_key(term, budget.max_content_ids_per_term);
                if let Some(cached) = cache.get(&key) {
                    self.posting_cache_hits.fetch_add(1, Ordering::Relaxed);
                    if let Some(posting) = cached {
                        postings.push(posting);
                    }
                } else {
                    self.posting_cache_misses.fetch_add(1, Ordering::Relaxed);
                    misses.push(term.clone());
                }
            }
        }

        for term in misses {
            cancellation.check()?;
            let (posting, truncated) = self.content.posting_for_term_limit_checked(
                &term,
                budget.max_content_ids_per_term,
                || cancellation.check(),
            )?;
            if !truncated {
                self.posting_cache_lock().insert(
                    posting_cache_key(&term, budget.max_content_ids_per_term),
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

    fn postings_for_terms_in_volume(
        &self,
        terms: Vec<String>,
        volume: VolumeId,
        budget: SearchLookupBudget,
        cancellation: &Cancellation,
    ) -> Result<Vec<ContentPosting>> {
        let mut selected = BTreeSet::new();
        for term in terms {
            cancellation.check()?;
            let term = canonical_query_term_checked(&term, || cancellation.check())?;
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
            let mut cache = self.posting_cache_lock();
            for term in &selected {
                cancellation.check()?;
                let key = volume_posting_cache_key(term, volume, budget.max_content_ids_per_term);
                if let Some(cached) = cache.get(&key) {
                    self.posting_cache_hits.fetch_add(1, Ordering::Relaxed);
                    if let Some(posting) = cached {
                        postings.push(posting);
                    }
                } else {
                    self.posting_cache_misses.fetch_add(1, Ordering::Relaxed);
                    misses.push(term.clone());
                }
            }
        }

        let loaded = self
            .content
            .postings_for_terms_volume_limit_checked(
                &misses,
                volume,
                budget.max_content_ids_per_term,
                || cancellation.check(),
            )?
            .into_iter()
            .map(|limited| (limited.posting.term.clone(), limited))
            .collect::<HashMap<_, _>>();

        let mut cache = self.posting_cache_lock();
        for term in misses {
            cancellation.check()?;
            let key = volume_posting_cache_key(&term, volume, budget.max_content_ids_per_term);
            let limited = loaded.get(&term).cloned();
            if !limited.as_ref().is_some_and(|posting| posting.truncated) {
                cache.insert(key, limited.as_ref().map(|posting| posting.posting.clone()));
            }
            if let Some(LimitedContentPosting { posting, .. }) = limited {
                postings.push(posting);
            }
        }

        postings.sort_by(|left, right| left.term.cmp(&right.term));
        Ok(postings)
    }

    fn records_contains_scope(&self, scope: &SearchVolumeScope) -> bool {
        match scope {
            SearchVolumeScope::All => !self.records.is_empty(),
            SearchVolumeScope::Only(volumes) => volumes
                .iter()
                .any(|volume| self.records.contains_volume(*volume)),
        }
    }

    fn posting_cache_lock(&self) -> MutexGuard<'_, ContentPostingCache> {
        self.posting_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn live_from_postings(
        &self,
        postings: Vec<ContentPosting>,
        has_content_terms: bool,
        scope: &SearchVolumeScope,
        cancellation: &Cancellation,
    ) -> Result<(LiveIndex, ContentQueryLoadReport)> {
        cancellation.check()?;
        let candidate_ids = content_candidate_ids_cancellable(&postings, cancellation)?;
        let has_content_postings = !postings.is_empty();
        let full_hydration =
            !has_content_terms || (has_content_postings && candidate_ids.is_empty());
        let candidate_count = candidate_ids.len();
        let (records, missing) = if full_hydration {
            self.hydrate_records_in_scope(scope, cancellation)?
        } else {
            self.hydrate_record_ids(candidate_ids, cancellation)?
        };
        cancellation.check()?;

        let content_keys = postings.len();
        let (live, _, _, _, _, _, _) = LiveIndex::from_records_with_sidecars(
            records,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            postings,
        );
        let records_loaded = live.indexed_records();
        Ok((
            live,
            ContentQueryLoadReport {
                content_keys,
                candidate_ids: candidate_count,
                records_loaded,
                records_missing: missing,
                full_hydration,
            },
        ))
    }

    fn hydrate_records_in_scope(
        &self,
        scope: &SearchVolumeScope,
        cancellation: &Cancellation,
    ) -> Result<(Vec<FileRecord>, usize)> {
        match scope {
            SearchVolumeScope::All => self.hydrate_all_records(cancellation),
            SearchVolumeScope::Only(volumes) => {
                let mut records = Vec::new();
                for volume in volumes {
                    cancellation.check()?;
                    records.extend(
                        self.records
                            .records_for_volume_checked(*volume, || cancellation.check())?,
                    );
                }
                cancellation.check()?;
                Ok((records, 0))
            }
        }
    }

    fn hydrate_all_records(&self, cancellation: &Cancellation) -> Result<(Vec<FileRecord>, usize)> {
        let mut records = Vec::with_capacity(self.records.len());
        for index in 0..self.records.len() {
            cancellation.check()?;
            records.push(
                self.records
                    .record_checked(index, || cancellation.check())?,
            );
        }
        Ok((records, 0))
    }

    fn hydrate_record_ids(
        &self,
        ids: BTreeSet<FileId>,
        cancellation: &Cancellation,
    ) -> Result<(Vec<FileRecord>, usize)> {
        if ids.is_empty() {
            return Ok((Vec::new(), 0));
        }

        let mut records_by_id = HashMap::new();
        let mut misses = Vec::new();
        {
            let mut cache = self.record_cache_lock();
            for id in &ids {
                cancellation.check()?;
                if let Some(record) = cache.get(*id) {
                    self.record_cache_hits.fetch_add(1, Ordering::Relaxed);
                    records_by_id.insert(*id, record);
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
        {
            let mut cache = self.record_cache_lock();
            for record in batch.records {
                cancellation.check()?;
                cache.insert(record.id, record.clone());
                records_by_id.insert(record.id, record);
            }
        }

        let records = ids
            .into_iter()
            .filter_map(|id| records_by_id.remove(&id))
            .collect::<Vec<_>>();
        Ok((records, missing))
    }

    fn record_cache_lock(&self) -> MutexGuard<'_, ContentRecordCache> {
        self.record_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn result_cache_lock(&self) -> MutexGuard<'_, ContentResultCache> {
        self.result_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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

fn escape_tsv_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn canonical_query_term_checked(
    term: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<String> {
    check_control()?;
    let mut canonical = String::new();
    for (index, ch) in term.trim().chars().enumerate() {
        if index.is_multiple_of(CONTENT_QUERY_TERM_CHECK_STRIDE) {
            check_control()?;
        }
        canonical.extend(ch.to_lowercase());
    }
    check_control()?;
    Ok(canonical)
}

fn empty_content_query_session_report() -> ContentQuerySessionReport {
    ContentQuerySessionReport {
        load: ContentQueryLoadReport::default(),
        search: SearchQueryReport {
            hits: Vec::new(),
            lookup: SearchLookupTelemetry::default(),
        },
        posting_cache_hits: 0,
        posting_cache_misses: 0,
        record_cache_hits: 0,
        record_cache_misses: 0,
        result_cache_hits: 0,
        result_cache_misses: 0,
    }
}

fn posting_cache_key(term: &str, limit: usize) -> String {
    format!("{limit}:{term}")
}

fn volume_posting_cache_key(term: &str, volume: VolumeId, limit: usize) -> String {
    format!("v:{}:{limit}:{term}", volume.0)
}

fn content_query_result_cache_key(
    query: &SearchQuery,
    limit: usize,
    scope: &SearchVolumeScope,
    budget: SearchLookupBudget,
) -> String {
    format!(
        "{}\0{}\0{}\0{}",
        query.canonical_cache_key(),
        limit,
        search_volume_scope_cache_key(scope),
        budget.max_content_ids_per_term
    )
}

fn search_volume_scope_cache_key(scope: &SearchVolumeScope) -> String {
    match scope {
        SearchVolumeScope::All => "all".to_string(),
        SearchVolumeScope::Only(volumes) => {
            let mut key = format!("only:{}", volumes.len());
            for volume in volumes {
                key.push(':');
                key.push_str(&volume.0.to_string());
            }
            key
        }
    }
}

fn scope_excludes_all(scope: &SearchVolumeScope) -> bool {
    matches!(scope, SearchVolumeScope::Only(volumes) if volumes.is_empty())
}

#[derive(Debug)]
struct ContentPostingCache {
    capacity: usize,
    next_generation: u64,
    order: VecDeque<(u64, String)>,
    values: HashMap<String, ContentPostingCacheEntry>,
}

#[derive(Debug)]
struct ContentPostingCacheEntry {
    generation: u64,
    posting: Option<ContentPosting>,
}

impl ContentPostingCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            next_generation: 0,
            order: VecDeque::with_capacity(capacity),
            values: HashMap::new(),
        }
    }

    fn get(&mut self, key: &str) -> Option<Option<ContentPosting>> {
        let generation = next_cache_generation(&mut self.next_generation);
        let posting = {
            let entry = self.values.get_mut(key)?;
            entry.generation = generation;
            entry.posting.clone()
        };
        self.order.push_back((generation, key.to_string()));
        self.compact_stale_order_if_needed();
        Some(posting)
    }

    fn insert(&mut self, key: String, posting: Option<ContentPosting>) {
        if self.capacity == 0 {
            return;
        }
        let generation = next_cache_generation(&mut self.next_generation);
        self.order.push_back((generation, key.clone()));
        self.values.insert(
            key,
            ContentPostingCacheEntry {
                generation,
                posting,
            },
        );
        self.evict_over_capacity();
        self.compact_stale_order_if_needed();
    }

    fn evict_over_capacity(&mut self) {
        while self.values.len() > self.capacity {
            let Some(expired) = self.order.pop_front() else {
                break;
            };
            if self
                .values
                .get(&expired.1)
                .is_some_and(|entry| entry.generation == expired.0)
            {
                self.values.remove(&expired.1);
            }
        }
    }

    fn compact_stale_order_if_needed(&mut self) {
        let max_order = self.capacity.saturating_mul(4).max(self.capacity + 1);
        if self.order.len() <= max_order {
            return;
        }
        let mut live = self
            .values
            .iter()
            .map(|(key, entry)| (entry.generation, key.clone()))
            .collect::<Vec<_>>();
        live.sort_by_key(|(generation, _)| *generation);
        self.order = live.into_iter().collect();
    }
}

#[derive(Debug)]
struct ContentRecordCache {
    capacity: usize,
    next_generation: u64,
    order: VecDeque<(u64, FileId)>,
    values: HashMap<FileId, ContentRecordCacheEntry>,
}

#[derive(Debug)]
struct ContentRecordCacheEntry {
    generation: u64,
    record: FileRecord,
}

#[derive(Debug)]
struct ContentResultCache {
    capacity: usize,
    next_generation: u64,
    order: VecDeque<(u64, String)>,
    values: HashMap<String, ContentResultCacheEntry>,
}

#[derive(Debug)]
struct ContentResultCacheEntry {
    generation: u64,
    report: ContentQuerySessionReport,
}

impl ContentResultCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            next_generation: 0,
            order: VecDeque::with_capacity(capacity),
            values: HashMap::new(),
        }
    }

    fn get(&mut self, key: &str) -> Option<ContentQuerySessionReport> {
        let generation = next_cache_generation(&mut self.next_generation);
        let report = {
            let entry = self.values.get_mut(key)?;
            entry.generation = generation;
            entry.report.clone()
        };
        self.order.push_back((generation, key.to_string()));
        self.compact_stale_order_if_needed();
        Some(report)
    }

    fn insert(&mut self, key: String, report: ContentQuerySessionReport) {
        if self.capacity == 0 {
            return;
        }
        let generation = next_cache_generation(&mut self.next_generation);
        self.order.push_back((generation, key.clone()));
        self.values
            .insert(key, ContentResultCacheEntry { generation, report });
        self.evict_over_capacity();
        self.compact_stale_order_if_needed();
    }

    fn evict_over_capacity(&mut self) {
        while self.values.len() > self.capacity {
            let Some(expired) = self.order.pop_front() else {
                break;
            };
            if self
                .values
                .get(&expired.1)
                .is_some_and(|entry| entry.generation == expired.0)
            {
                self.values.remove(&expired.1);
            }
        }
    }

    fn compact_stale_order_if_needed(&mut self) {
        let max_order = self.capacity.saturating_mul(4).max(self.capacity + 1);
        if self.order.len() <= max_order {
            return;
        }
        let mut live = self
            .values
            .iter()
            .map(|(key, entry)| (entry.generation, key.clone()))
            .collect::<Vec<_>>();
        live.sort_by_key(|(generation, _)| *generation);
        self.order = live.into_iter().collect();
    }

    fn clear(&mut self) {
        self.order.clear();
        self.values.clear();
    }

    fn len(&self) -> usize {
        self.values.len()
    }
}

impl ContentRecordCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            next_generation: 0,
            order: VecDeque::with_capacity(capacity),
            values: HashMap::new(),
        }
    }

    fn get(&mut self, id: FileId) -> Option<FileRecord> {
        let generation = next_cache_generation(&mut self.next_generation);
        let record = {
            let entry = self.values.get_mut(&id)?;
            entry.generation = generation;
            entry.record.clone()
        };
        self.order.push_back((generation, id));
        self.compact_stale_order_if_needed();
        Some(record)
    }

    fn insert(&mut self, id: FileId, record: FileRecord) {
        if self.capacity == 0 {
            return;
        }
        let generation = next_cache_generation(&mut self.next_generation);
        self.order.push_back((generation, id));
        self.values
            .insert(id, ContentRecordCacheEntry { generation, record });
        self.evict_over_capacity();
        self.compact_stale_order_if_needed();
    }

    fn evict_over_capacity(&mut self) {
        while self.values.len() > self.capacity {
            let Some(expired) = self.order.pop_front() else {
                break;
            };
            if self
                .values
                .get(&expired.1)
                .is_some_and(|entry| entry.generation == expired.0)
            {
                self.values.remove(&expired.1);
            }
        }
    }

    fn compact_stale_order_if_needed(&mut self) {
        let max_order = self.capacity.saturating_mul(4).max(self.capacity + 1);
        if self.order.len() <= max_order {
            return;
        }
        let mut live = self
            .values
            .iter()
            .map(|(id, entry)| (entry.generation, *id))
            .collect::<Vec<_>>();
        live.sort_by_key(|(generation, _)| *generation);
        self.order = live.into_iter().collect();
    }
}

fn next_cache_generation(next_generation: &mut u64) -> u64 {
    let generation = *next_generation;
    *next_generation = next_generation.wrapping_add(1);
    generation
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_jobs::Cancellation;
    use gfm_store::{write_content_postings, write_records};
    use gfm_types::{ContentPositions, FileKind, GfmError, VolumeId};
    use std::fs;
    use std::panic::{self, AssertUnwindSafe};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn content_session_recovers_poisoned_posting_cache() {
        let fixture = ContentSessionFixture::new("posting-cache");
        let session = fixture.session();

        poison_posting_cache(&session);
        let report = session.search("needle", 5).unwrap();

        assert_eq!(report.search.hits.len(), 1);
        assert_eq!(report.search.hits[0].record.name, "Needle.md");
        assert_eq!(report.posting_cache_misses, 1);
        assert_eq!(report.record_cache_misses, 1);
    }

    #[test]
    fn content_session_recovers_poisoned_record_cache() {
        let fixture = ContentSessionFixture::new("record-cache");
        let session = fixture.session();
        let first = session.search("needle", 5).unwrap();
        assert_eq!(first.search.hits.len(), 1);

        poison_record_cache(&session);
        let second = session.search("needle", 6).unwrap();

        assert_eq!(second.search.hits.len(), 1);
        assert_eq!(second.search.hits[0].record.name, "Needle.md");
        assert_eq!(second.posting_cache_hits, 1);
        assert_eq!(second.record_cache_hits, 1);
        assert_eq!(second.result_cache_hits, 0);
        assert_eq!(second.result_cache_misses, 1);
    }

    #[test]
    fn content_session_reuses_normalized_query_results() {
        let fixture = ContentSessionFixture::new("result-cache");
        let session = fixture.session();

        let first = session.search("  Needle  ", 5).unwrap();
        let second = session.search("needle", 5).unwrap();

        assert_eq!(first.search.hits, second.search.hits);
        assert_eq!(first.posting_cache_hits, 0);
        assert_eq!(first.posting_cache_misses, 1);
        assert_eq!(first.record_cache_hits, 0);
        assert_eq!(first.record_cache_misses, 1);
        assert_eq!(first.result_cache_hits, 0);
        assert_eq!(first.result_cache_misses, 1);
        assert_eq!(second.posting_cache_hits, 0);
        assert_eq!(second.posting_cache_misses, 0);
        assert_eq!(second.record_cache_hits, 0);
        assert_eq!(second.record_cache_misses, 0);
        assert_eq!(second.result_cache_hits, 1);
        assert_eq!(second.result_cache_misses, 0);
        assert_eq!(session.result_cache_telemetry(), (1, 1));
    }

    #[test]
    fn content_session_volume_scope_batches_content_terms_and_isolates_caches() {
        let root = temp_dir("gfm-content-session-volume-scope");
        let records = root.join("records.gfmidx");
        let first_content = root.join("first.gfmcontent");
        let second_content = root.join("second.gfmcontent");
        let volume_one = VolumeId(1);
        let volume_two = VolumeId(2);
        let first_id = FileId::new(volume_one, 100);
        let second_id = FileId::new(volume_two, 200);
        write_records(
            &records,
            &[
                FileRecord {
                    id: first_id,
                    path: root.join("one.md"),
                    name: "one.md".to_string(),
                    ..record(first_id)
                },
                FileRecord {
                    id: second_id,
                    path: root.join("two.md"),
                    name: "two.md".to_string(),
                    ..record(second_id)
                },
            ],
        )
        .unwrap();
        write_content_postings(
            &first_content,
            &[ContentPosting {
                term: "shared".to_string(),
                ids: vec![first_id],
                positions: vec![ContentPositions {
                    id: first_id,
                    positions: vec![1],
                }],
            }],
        )
        .unwrap();
        write_content_postings(
            &second_content,
            &[ContentPosting {
                term: "shared".to_string(),
                ids: vec![second_id],
                positions: vec![ContentPositions {
                    id: second_id,
                    positions: vec![2],
                }],
            }],
        )
        .unwrap();
        let session =
            ContentIndexQuerySession::open_set(&records, [&first_content, &second_content])
                .unwrap();

        let scoped = session
            .search_with_volume_scope("shared", 10, &SearchVolumeScope::only([volume_two]))
            .unwrap();
        let cached_scoped = session
            .search_with_volume_scope("SHARED", 10, &SearchVolumeScope::only([volume_two]))
            .unwrap();
        let all = session.search("shared", 10).unwrap();
        let empty = session
            .search_with_volume_scope("shared", 10, &SearchVolumeScope::only([VolumeId(99)]))
            .unwrap();

        assert_eq!(scoped.search.hits.len(), 1);
        assert_eq!(scoped.search.hits[0].record.id, second_id);
        assert_eq!(scoped.load.content_keys, 1);
        assert_eq!(scoped.load.candidate_ids, 1);
        assert_eq!(scoped.load.records_loaded, 1);
        assert_eq!(scoped.load.records_missing, 0);
        assert!(!scoped.load.full_hydration);
        assert_eq!(scoped.posting_cache_hits, 0);
        assert_eq!(scoped.posting_cache_misses, 1);
        assert_eq!(scoped.record_cache_hits, 0);
        assert_eq!(scoped.record_cache_misses, 1);
        assert_eq!(scoped.search.hits, cached_scoped.search.hits);
        assert_eq!(scoped.result_cache_hits, 0);
        assert_eq!(scoped.result_cache_misses, 1);
        assert_eq!(cached_scoped.result_cache_hits, 1);
        assert_eq!(cached_scoped.result_cache_misses, 0);
        assert_eq!(all.search.hits.len(), 2);
        assert_eq!(all.result_cache_hits, 0);
        assert_eq!(all.result_cache_misses, 1);
        assert_eq!(empty.search.hits.len(), 0);
        assert_eq!(empty.posting_cache_misses, 0);
        assert_eq!(empty.record_cache_misses, 0);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn content_session_metadata_only_volume_scope_hydrates_only_admitted_records() {
        let root = temp_dir("gfm-content-session-metadata-volume-scope");
        let records = root.join("records.gfmidx");
        let content = root.join("content.gfmcontent");
        let volume_one = VolumeId(1);
        let volume_two = VolumeId(2);
        let first_id = FileId::new(volume_one, 100);
        let second_id = FileId::new(volume_two, 200);
        write_records(
            &records,
            &[
                FileRecord {
                    id: first_id,
                    path: root.join("one.md"),
                    name: "one.md".to_string(),
                    ..record(first_id)
                },
                FileRecord {
                    id: second_id,
                    path: root.join("two.md"),
                    name: "two.md".to_string(),
                    ..record(second_id)
                },
            ],
        )
        .unwrap();
        write_content_postings(&content, &[]).unwrap();
        let session = ContentIndexQuerySession::open_content(&records, &content).unwrap();

        let scoped = session
            .search_with_volume_scope("kind:file", 10, &SearchVolumeScope::only([volume_two]))
            .unwrap();

        assert_eq!(scoped.search.hits.len(), 1);
        assert_eq!(scoped.search.hits[0].record.id, second_id);
        assert_eq!(scoped.load.content_keys, 0);
        assert_eq!(scoped.load.candidate_ids, 0);
        assert_eq!(scoped.load.records_loaded, 1);
        assert_eq!(scoped.load.records_missing, 0);
        assert!(scoped.load.full_hydration);
        assert_eq!(scoped.posting_cache_hits, 0);
        assert_eq!(scoped.posting_cache_misses, 0);
        assert_eq!(scoped.record_cache_hits, 0);
        assert_eq!(scoped.record_cache_misses, 0);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn provider_metadata_invalidation_clears_content_query_results() {
        let fixture = ContentSessionFixture::new("provider-cache-clear");
        let session = fixture.session();
        let first = session.search("needle", 5).unwrap();
        let cached = session.search("needle", 5).unwrap();
        assert_eq!(first.search.hits, cached.search.hits);
        assert_eq!(cached.result_cache_hits, 1);

        let provider = ProviderMetadataInvalidationReport::from_provider_transition(
            fixture.root.join("Needle.md"),
            "downloaded",
            "evicted",
            true,
            true,
            "fileprovider-state-changed",
        );
        let invalidation = session.apply_provider_metadata_invalidation(&provider);
        let after = session.search("needle", 5).unwrap();

        assert!(invalidation.invalidated);
        assert_eq!(invalidation.result_entries_before, 1);
        assert_eq!(invalidation.result_entries_after, 0);
        assert_eq!(invalidation.reason, "provider-metadata-state-changed");
        assert_eq!(
            invalidation.as_tsv(),
            format!(
                "content-query-cache-invalidation\t{}\tinvalidated=true\tresult-entries-before=1\tresult-entries-after=0\treason=provider-metadata-state-changed",
                fixture.root.join("Needle.md").display()
            )
        );
        assert_eq!(after.result_cache_hits, 0);
        assert_eq!(after.result_cache_misses, 1);
        assert_eq!(after.posting_cache_hits, 1);
    }

    #[test]
    fn provider_metadata_noop_preserves_content_query_results() {
        let fixture = ContentSessionFixture::new("provider-cache-noop");
        let session = fixture.session();
        let first = session.search("needle", 5).unwrap();
        let cached = session.search("needle", 5).unwrap();
        assert_eq!(first.search.hits, cached.search.hits);
        assert_eq!(cached.result_cache_hits, 1);

        let provider = ProviderMetadataInvalidationReport::from_provider_transition(
            fixture.root.join("Needle.md"),
            "downloaded",
            "downloaded",
            true,
            false,
            "fileprovider-state-unchanged",
        );
        let invalidation = session.apply_provider_metadata_invalidation(&provider);
        let after = session.search("needle", 5).unwrap();

        assert!(!invalidation.invalidated);
        assert_eq!(invalidation.result_entries_before, 1);
        assert_eq!(invalidation.result_entries_after, 1);
        assert_eq!(invalidation.reason, "provider-state-unchanged");
        assert_eq!(after.result_cache_hits, 1);
        assert_eq!(after.result_cache_misses, 0);
    }

    #[test]
    fn content_posting_cache_refreshes_recency_on_hit() {
        let first = ContentPosting {
            term: "first".to_string(),
            ids: vec![FileId::new(VolumeId(1), 1)],
            positions: Vec::new(),
        };
        let second = ContentPosting {
            term: "second".to_string(),
            ids: vec![FileId::new(VolumeId(1), 2)],
            positions: Vec::new(),
        };
        let third = ContentPosting {
            term: "third".to_string(),
            ids: vec![FileId::new(VolumeId(1), 3)],
            positions: Vec::new(),
        };
        let mut cache = ContentPostingCache::new(2);
        cache.insert("first".to_string(), Some(first.clone()));
        cache.insert("second".to_string(), Some(second));

        assert_eq!(cache.get("first"), Some(Some(first.clone())));
        cache.insert("third".to_string(), Some(third));

        assert_eq!(cache.get("first"), Some(Some(first)));
        assert_eq!(cache.get("second"), None);
    }

    #[test]
    fn content_posting_cache_hot_hits_compact_stale_recency_entries() {
        let first = ContentPosting {
            term: "first".to_string(),
            ids: vec![FileId::new(VolumeId(1), 1)],
            positions: Vec::new(),
        };
        let second = ContentPosting {
            term: "second".to_string(),
            ids: vec![FileId::new(VolumeId(1), 2)],
            positions: Vec::new(),
        };
        let third = ContentPosting {
            term: "third".to_string(),
            ids: vec![FileId::new(VolumeId(1), 3)],
            positions: Vec::new(),
        };
        let mut cache = ContentPostingCache::new(2);
        cache.insert("first".to_string(), Some(first.clone()));
        cache.insert("second".to_string(), Some(second));

        for _ in 0..32 {
            assert_eq!(cache.get("first"), Some(Some(first.clone())));
        }

        assert!(cache.order.len() <= 8, "{:?}", cache.order);
        cache.insert("third".to_string(), Some(third));
        assert_eq!(cache.get("first"), Some(Some(first)));
        assert_eq!(cache.get("second"), None);
    }

    #[test]
    fn content_result_cache_refreshes_recency_on_hit() {
        let mut first = empty_content_query_session_report();
        first.result_cache_misses = 11;
        let mut second = empty_content_query_session_report();
        second.result_cache_misses = 22;
        let mut third = empty_content_query_session_report();
        third.result_cache_misses = 33;
        let mut cache = ContentResultCache::new(2);
        cache.insert("first".to_string(), first.clone());
        cache.insert("second".to_string(), second);

        assert_eq!(cache.get("first"), Some(first.clone()));
        cache.insert("third".to_string(), third);

        assert_eq!(cache.get("first"), Some(first));
        assert_eq!(cache.get("second"), None);
    }

    #[test]
    fn content_result_cache_hot_hits_compact_stale_recency_entries() {
        let mut first = empty_content_query_session_report();
        first.result_cache_misses = 11;
        let mut second = empty_content_query_session_report();
        second.result_cache_misses = 22;
        let mut third = empty_content_query_session_report();
        third.result_cache_misses = 33;
        let mut cache = ContentResultCache::new(2);
        cache.insert("first".to_string(), first.clone());
        cache.insert("second".to_string(), second);

        for _ in 0..32 {
            assert_eq!(cache.get("first"), Some(first.clone()));
        }

        assert!(cache.order.len() <= 8, "{:?}", cache.order);
        cache.insert("third".to_string(), third);
        assert_eq!(cache.get("first"), Some(first));
        assert_eq!(cache.get("second"), None);
    }

    #[test]
    fn content_record_cache_refreshes_hot_records() {
        let first = FileId::new(VolumeId(1), 1);
        let second = FileId::new(VolumeId(1), 2);
        let third = FileId::new(VolumeId(1), 3);
        let mut cache = ContentRecordCache::new(2);
        cache.insert(first, record(first));
        cache.insert(second, record(second));

        for _ in 0..32 {
            assert_eq!(cache.get(first).unwrap().id, first);
        }

        assert!(cache.order.len() <= 8, "{:?}", cache.order);
        cache.insert(third, record(third));
        assert_eq!(cache.get(first).unwrap().id, first);
        assert!(cache.get(second).is_none());
        assert_eq!(cache.get(third).unwrap().id, third);
    }

    #[test]
    fn content_session_empty_and_zero_limit_queries_skip_cache_work() {
        let fixture = ContentSessionFixture::new("empty-query");
        let session = fixture.session();

        let empty = session.search("   ", 5).unwrap();
        let zero_limit = session.search("needle", 0).unwrap();

        assert!(empty.search.hits.is_empty());
        assert_eq!(empty.load, ContentQueryLoadReport::default());
        assert_eq!(zero_limit, empty);
        assert_eq!(session.posting_cache_telemetry(), (0, 0));
        assert_eq!(session.record_cache_telemetry(), (0, 0));
        assert_eq!(session.result_cache_telemetry(), (0, 0));
    }

    #[test]
    fn content_session_honors_pre_cancelled_queries_without_cache_work() {
        let fixture = ContentSessionFixture::new("pre-cancelled");
        let session = fixture.session();
        let cancellation = Cancellation::default();
        cancellation.cancel();

        let result = session.search_cancellable("needle", 5, &cancellation);

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert_eq!(session.posting_cache_telemetry(), (0, 0));
        assert_eq!(session.record_cache_telemetry(), (0, 0));
        assert_eq!(session.result_cache_telemetry(), (0, 0));
    }

    #[test]
    fn index_query_supersession_cancels_previous_content_query_token() {
        let fixture = ContentSessionFixture::new("supersession-content");
        let session = fixture.session();
        let supersession = IndexQuerySupersession::new();
        let previous = supersession.begin();

        let report = supersession.search_content(&session, "needle", 5).unwrap();

        assert!(matches!(previous.check(), Err(GfmError::Cancelled)));
        assert_eq!(report.search.hits.len(), 1);
        assert_eq!(report.search.hits[0].record.name, "Needle.md");
        assert_eq!(report.result_cache_hits, 0);
        assert_eq!(report.result_cache_misses, 1);
    }

    #[test]
    fn index_query_supersession_recovers_poisoned_active_lock() {
        let supersession = IndexQuerySupersession::new();
        let previous = supersession.begin();

        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            let _guard = supersession
                .active
                .lock()
                .expect("initial index query supersession lock");
            panic!("poison index query supersession lock");
        }));
        let next = supersession.begin();

        assert!(matches!(previous.check(), Err(GfmError::Cancelled)));
        assert!(next.check().is_ok());
    }

    #[test]
    fn content_candidate_expansion_honors_cancelled_tokens() {
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

    #[test]
    fn content_query_term_canonicalization_honors_checked_control() {
        let mut checks = 0usize;
        let result = canonical_query_term_checked(&"Needle".repeat(512), || {
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

    fn poison_posting_cache(session: &ContentIndexQuerySession) {
        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            let _guard = session
                .posting_cache
                .lock()
                .expect("initial content posting cache lock");
            panic!("poison content posting cache");
        }));
    }

    fn poison_record_cache(session: &ContentIndexQuerySession) {
        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            let _guard = session
                .record_cache
                .lock()
                .expect("initial content record cache lock");
            panic!("poison content record cache");
        }));
    }

    struct ContentSessionFixture {
        root: PathBuf,
        records: PathBuf,
        content: PathBuf,
    }

    impl ContentSessionFixture {
        fn new(name: &str) -> Self {
            let root = temp_dir(&format!("gfm-content-session-{name}"));
            let records = root.join("records.gfmidx");
            let content = root.join("content.gfmcontent");
            let id = FileId::new(VolumeId(1), 42);
            write_records(&records, &[record(id)]).unwrap();
            write_content_postings(
                &content,
                &[ContentPosting {
                    term: "needle".to_string(),
                    ids: vec![id],
                    positions: vec![ContentPositions {
                        id,
                        positions: vec![1],
                    }],
                }],
            )
            .unwrap();
            Self {
                root,
                records,
                content,
            }
        }

        fn session(&self) -> ContentIndexQuerySession {
            ContentIndexQuerySession::open_content(&self.records, &self.content).unwrap()
        }
    }

    impl Drop for ContentSessionFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn record(id: FileId) -> FileRecord {
        FileRecord {
            id,
            parent: None,
            path: PathBuf::from("/tmp/Needle.md"),
            name: "Needle.md".to_string(),
            kind: FileKind::File,
            len: 6,
            mode: 0o100644,
            owner: 501,
            group: 20,
            xattrs_digest: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: Vec::new(),
            finder_comment: None,
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
