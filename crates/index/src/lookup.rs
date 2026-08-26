use gfm_search::substring_candidate_grams;
use gfm_search::{
    SearchFuzzyPosting, SearchLookup, SearchLookupBudget, SearchLookupIds, SearchLookupTelemetry,
    SearchLookupTerms, SearchMetadataField, SearchMetadataPosting, SearchPrefixPosting,
    SearchQueryReport, SearchRecordColumns, SearchSubstringPosting,
};
use gfm_store::{
    MetadataField, MmapContentArchive, MmapFuzzyArchive, MmapMetadataArchive, MmapPrefixArchive,
    MmapRecordArchive, MmapRecordColumns, MmapSubstringArchive,
};
use gfm_types::{ContentPosting, FileId, FileRecord, Result};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::Path;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex, MutexGuard,
};

const SEARCH_ARCHIVE_LOOKUP_CACHE_CAPACITY: usize = 512;
const SIDECAR_RECORD_CACHE_CAPACITY: usize = 8192;
const SIDECAR_CONTENT_POSTING_CACHE_CAPACITY: usize = 512;

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
}

#[derive(Debug)]
pub struct SidecarIndexQuerySession {
    records: MmapRecordArchive,
    columns: MmapRecordColumns,
    metadata: MmapMetadataArchive,
    lookup: SearchArchiveLookup,
    substrings: MmapSubstringArchive,
    content: MmapContentArchive,
    content_cache: Mutex<LookupCache<Option<ContentPosting>>>,
    content_cache_hits: AtomicUsize,
    content_cache_misses: AtomicUsize,
    record_cache: Mutex<RecordCache>,
    record_cache_hits: AtomicUsize,
    record_cache_misses: AtomicUsize,
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
        let substrings = substrings.as_ref();
        Ok(Self {
            records: MmapRecordArchive::open(records)?,
            columns: MmapRecordColumns::open(columns)?,
            metadata: MmapMetadataArchive::open(metadata)?,
            lookup: SearchArchiveLookup::open(prefixes, substrings, fuzzy)?,
            substrings: MmapSubstringArchive::open(substrings)?,
            content: MmapContentArchive::open(content)?,
            content_cache: Mutex::new(LookupCache::new(SIDECAR_CONTENT_POSTING_CACHE_CAPACITY)),
            content_cache_hits: AtomicUsize::new(0),
            content_cache_misses: AtomicUsize::new(0),
            record_cache: Mutex::new(RecordCache::new(SIDECAR_RECORD_CACHE_CAPACITY)),
            record_cache_hits: AtomicUsize::new(0),
            record_cache_misses: AtomicUsize::new(0),
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

    pub fn search(&self, query: &str, limit: usize) -> Result<SidecarQuerySessionReport> {
        self.search_with_budget(query, limit, SearchLookupBudget::default())
    }

    pub fn search_with_budget(
        &self,
        query: &str,
        limit: usize,
        budget: SearchLookupBudget,
    ) -> Result<SidecarQuerySessionReport> {
        let content_hits_before = self.content_cache_hits.load(Ordering::Relaxed);
        let content_misses_before = self.content_cache_misses.load(Ordering::Relaxed);
        let parsed = gfm_search::SearchQuery::parse(query);
        let content_terms = parsed.content_candidate_terms();
        let content_postings = self
            .content_postings_for_terms(content_terms.clone(), budget.max_content_ids_per_term)?;
        let import = query_sidecar_imports_with_content_postings(
            &self.metadata,
            &self.lookup,
            &self.substrings,
            &parsed,
            content_terms,
            content_postings,
            budget,
        )?;
        let cache_hits_before = self.record_cache_hits.load(Ordering::Relaxed);
        let cache_misses_before = self.record_cache_misses.load(Ordering::Relaxed);
        let (live, hydration) = self.live_from_import(import)?;
        let search = live.search_with_lookup_budget(query, limit, &self.lookup, budget)?;
        Ok(SidecarQuerySessionReport {
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
        })
    }

    fn content_postings_for_terms(
        &self,
        terms: Vec<String>,
        limit_per_term: usize,
    ) -> Result<Vec<ContentPosting>> {
        if limit_per_term == 0 {
            return Ok(Vec::new());
        }

        let mut selected = BTreeSet::new();
        for term in terms {
            let term = term.trim().to_lowercase();
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
            let cache = self.content_cache_lock();
            for term in &selected {
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

        let loaded = self
            .content
            .postings_for_sorted_terms_limit(&misses, limit_per_term)?
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

    fn live_from_import(
        &self,
        import: SidecarQueryImport,
    ) -> Result<(crate::LiveIndex, SidecarRecordHydrationReport)> {
        let (records, missing) = if import.report.requires_full_record_hydration {
            self.hydrate_all_records()?
        } else {
            self.hydrate_record_ids(sidecar_candidate_ids(&import))?
        };
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

    fn hydrate_all_records(&self) -> Result<(Vec<HydratedRecord>, usize)> {
        let mut records = Vec::with_capacity(self.records.len());
        for index in 0..self.records.len() {
            let record = self.records.record(index)?;
            records.push(self.hydrate_record(record)?);
        }
        Ok((records, 0))
    }

    fn hydrate_record_ids(&self, ids: BTreeSet<FileId>) -> Result<(Vec<HydratedRecord>, usize)> {
        if ids.is_empty() {
            return Ok((Vec::new(), 0));
        }

        let mut hydrated_by_id = HashMap::new();
        let mut misses = Vec::new();
        {
            let cache = self.record_cache_lock();
            for id in &ids {
                if let Some(record) = cache.get(*id) {
                    self.record_cache_hits.fetch_add(1, Ordering::Relaxed);
                    hydrated_by_id.insert(*id, record);
                } else {
                    self.record_cache_misses.fetch_add(1, Ordering::Relaxed);
                    misses.push(*id);
                }
            }
        }

        let batch = self
            .records
            .records_for_sorted_ids(misses.iter().copied())?;
        let missing = batch.missing;
        let mut loaded = Vec::with_capacity(batch.records.len());
        for record in batch.records {
            let id = record.id;
            loaded.push((id, self.hydrate_record(record)?));
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

    fn hydrate_record(&self, record: FileRecord) -> Result<HydratedRecord> {
        let columns = self
            .columns
            .find(record.id)?
            .map(|column| SearchRecordColumns {
                id: column.id,
                name: column.name,
                path: column.path,
                extension: column.extension,
                tags: column.tags,
                comment: column.comment,
            });
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
        Self::open_with_capacity(
            prefixes,
            substrings,
            fuzzy,
            SEARCH_ARCHIVE_LOOKUP_CACHE_CAPACITY,
        )
    }

    fn open_with_capacity(
        prefixes: impl AsRef<Path>,
        substrings: impl AsRef<Path>,
        fuzzy: impl AsRef<Path>,
        cache_capacity: usize,
    ) -> Result<SearchArchiveLookup> {
        Ok(Self {
            prefixes: MmapPrefixArchive::open(prefixes)?,
            substrings: MmapSubstringArchive::open(substrings)?,
            fuzzy: MmapFuzzyArchive::open(fuzzy)?,
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

    fn prefix_postings_bounded<I, S>(
        &self,
        prefixes: I,
        limit: usize,
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
            let prefix = prefix.as_ref();
            if !prefix.is_empty() {
                selected.insert(prefix.to_string());
            }
        }

        let mut postings = Vec::with_capacity(selected.len());
        let mut misses = Vec::new();
        {
            let cache = self.prefix_cache_lock();
            for prefix in &selected {
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

        let loaded = self
            .prefixes
            .postings_for_sorted_prefixes_limit(&misses, limit)?
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

    pub(crate) fn fuzzy_postings_bounded<I, S>(
        &self,
        keys: I,
        limit: usize,
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
            let key = key.as_ref();
            if !key.is_empty() {
                selected.insert(key.to_string());
            }
        }

        let mut postings = Vec::with_capacity(selected.len());
        let mut misses = Vec::new();
        {
            let cache = self.fuzzy_cache_lock();
            for key in &selected {
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

        let loaded = self
            .fuzzy
            .postings_for_sorted_keys_limit(&misses, limit)?
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
        content_terms,
        content,
        budget,
    )
}

fn query_sidecar_imports_with_content_postings(
    metadata: &MmapMetadataArchive,
    lookup: &SearchArchiveLookup,
    substrings: &MmapSubstringArchive,
    parsed: &gfm_search::SearchQuery,
    content_terms: Vec<String>,
    content: Vec<ContentPosting>,
    budget: SearchLookupBudget,
) -> Result<SidecarQueryImport> {
    let comment_terms = parsed.comment_candidate_terms();
    let tag_terms = parsed.tag_candidate_terms();
    let prefix_terms = parsed.prefix_candidate_terms();
    let substring_grams = bounded_substring_grams(&content_terms, budget);
    let fuzzy_keys = parsed
        .fuzzy_candidate_keys()
        .into_iter()
        .take(budget.max_fuzzy_keys_per_term)
        .collect::<Vec<_>>();
    let mut candidate_ids = BTreeSet::new();

    let mut selected_metadata = metadata.postings_for_limit(
        MetadataField::Comment,
        comment_terms,
        budget.max_metadata_ids_per_term,
    )?;
    selected_metadata.extend(metadata.postings_for_limit(
        MetadataField::Tag,
        tag_terms.clone(),
        budget.max_metadata_ids_per_term,
    )?);
    let metadata = selected_metadata
        .into_iter()
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

    let substrings = substrings
        .postings_for_limit(substring_grams, budget.max_substring_ids_per_gram)?
        .into_iter()
        .map(|posting| {
            candidate_ids.extend(posting.ids.iter().copied());
            SearchSubstringPosting {
                gram: posting.gram,
                ids: posting.ids,
            }
        })
        .collect::<Vec<_>>();

    let mut prefix_candidates = prefix_terms.clone();
    let mut fuzzy_candidate_terms = BTreeSet::new();
    let fuzzy = lookup
        .fuzzy_postings_bounded(fuzzy_keys, budget.max_fuzzy_terms_per_key)?
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

    let prefixes =
        lookup.prefix_postings_bounded(prefix_candidates, budget.max_prefix_ids_per_term)?;
    for posting in &prefixes {
        candidate_ids.extend(posting.ids.iter().copied());
    }

    for posting in &content {
        candidate_ids.extend(posting.ids.iter().copied());
        candidate_ids.extend(posting.positions.iter().map(|positions| positions.id));
    }

    let has_positive_anchor = !content_terms.is_empty()
        || !tag_terms.is_empty()
        || !metadata.is_empty()
        || !prefixes.is_empty()
        || !substrings.is_empty()
        || !fuzzy.is_empty()
        || !content.is_empty();
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
            content_postings: content.len(),
            candidate_ids: candidate_ids.len(),
            requires_full_record_hydration,
        },
        metadata,
        prefixes,
        substrings,
        fuzzy,
        content,
    })
}

pub(crate) fn sidecar_candidate_ids(import: &SidecarQueryImport) -> BTreeSet<FileId> {
    let mut ids = BTreeSet::new();
    for posting in &import.metadata {
        ids.extend(posting.ids.iter().copied());
    }
    for posting in &import.prefixes {
        ids.extend(posting.ids.iter().copied());
    }
    for posting in &import.substrings {
        ids.extend(posting.ids.iter().copied());
    }
    for posting in &import.content {
        ids.extend(posting.ids.iter().copied());
        ids.extend(posting.positions.iter().map(|positions| positions.id));
    }
    ids
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

fn bounded_posting_cache_key(term: &str, limit: usize) -> String {
    format!("{limit}:{term}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_search::SearchLookup;
    use gfm_store::{
        fuzzy_postings_from_records, metadata_postings_from_records, prefix_postings_from_records,
        substring_postings_from_records, write_content_postings, write_fuzzy_postings,
        write_metadata_postings, write_prefix_postings, write_record_columns, write_records,
        write_substring_postings,
    };
    use gfm_types::{ContentPositions, FileKind, VolumeId};
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
        let second = session.search("finderlatency", 5).unwrap();

        assert_eq!(second.search.hits.len(), 1);
        assert_eq!(second.search.hits[0].record.id, fixture.record.id);
        assert_eq!(second.content_cache_hits, 1);
        assert_eq!(second.record_cache_hits, 1);
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
            let root = temp_dir(&format!("gfm-sidecar-poison-{name}"));
            let records = root.join("records.gfmidx");
            let columns = root.join("columns.gfmcols");
            let metadata = root.join("metadata.gfmmeta");
            let prefixes = root.join("prefixes.gfmprefix");
            let substrings = root.join("substrings.gfmsubstr");
            let fuzzy = root.join("fuzzy.gfmfuzzy");
            let content = root.join("content.gfmcontent");
            let record = record(FileId::new(VolumeId(7), 42));
            let record_set = vec![record.clone()];

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
                &[ContentPosting {
                    term: "finderlatency".to_string(),
                    ids: vec![record.id],
                    positions: vec![ContentPositions {
                        id: record.id,
                        positions: vec![1],
                    }],
                }],
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

    fn get(&self, key: &str) -> Option<V> {
        self.values.get(key).cloned()
    }

    fn insert(&mut self, key: String, value: V) {
        if self.capacity == 0 {
            return;
        }
        if !self.values.contains_key(&key) {
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

    #[cfg(test)]
    fn len(&self) -> usize {
        self.values.len()
    }
}
