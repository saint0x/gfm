mod columns;
mod fuzzy;
mod intent;
mod query;
mod ranking;
mod session;
mod shard;

pub use columns::SearchRecordColumns;
use columns::{filter_matches_columns, RecordColumns};
use fuzzy::{bounded_levenshtein, deletion_keys};
use intent::{term_matches_intent, QueryIntent};
use query::{normalize, tokenize};
pub use query::{
    DateComparison, DateField, QueryExpr, QueryFilter, QueryKind, QueryProximity, QueryScope,
    SearchQuery, SizeComparison,
};
use ranking::{
    capped_frequency, count_term, filter_kind_matches, kind_score, recency_score, RankAccumulator,
    CONTENT, CONTENT_FREQUENCY, EXACT_NAME, EXTENSION, FUZZY_NAME, KIND_MATCH, NAME_FREQUENCY,
    NAME_TOKEN, PATH_COMPONENT, PATH_FREQUENCY, PHRASE, PREFIX_NAME, PROXIMITY, SUBSTRING_NAME,
    TAG, USER_PINNED,
};
pub use session::SearchSupersession;
pub use shard::ShardedSearchIndex;

use gfm_jobs::Cancellation;
use gfm_types::{
    ContentPositions, ContentPosting, FileId, FileKind, FileRecord, MatchReason, SearchHit,
};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap};

const FUZZY_MIN_TERM_LEN: usize = 2;
const FUZZY_MAX_TERM_LEN: usize = 32;
const PREFIX_MIN_TERM_LEN: usize = 1;
const PREFIX_MAX_TERM_LEN: usize = 32;
const SUBSTRING_GRAM_CHARS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchLookupBudget {
    pub max_prefix_ids_per_term: usize,
    pub min_archive_prefix_chars: usize,
    pub max_substring_grams_per_term: usize,
    pub max_substring_ids_per_gram: usize,
    pub max_fuzzy_keys_per_term: usize,
    pub max_fuzzy_terms_per_key: usize,
    pub max_fuzzy_candidates_per_term: usize,
    pub max_metadata_ids_per_term: usize,
    pub max_content_ids_per_term: usize,
}

impl Default for SearchLookupBudget {
    fn default() -> Self {
        Self {
            max_prefix_ids_per_term: 4096,
            min_archive_prefix_chars: 2,
            max_substring_grams_per_term: 16,
            max_substring_ids_per_gram: 4096,
            max_fuzzy_keys_per_term: 96,
            max_fuzzy_terms_per_key: 512,
            max_fuzzy_candidates_per_term: 4096,
            max_metadata_ids_per_term: 4096,
            max_content_ids_per_term: 4096,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchLookupTelemetry {
    pub prefix_terms: usize,
    pub prefix_lookup_requests: usize,
    pub prefix_lookup_ids: usize,
    pub prefix_candidate_ids: usize,
    pub prefix_cache_hits: usize,
    pub prefix_cache_misses: usize,
    pub prefix_cutoff_terms: usize,
    pub prefix_truncated_terms: usize,
    pub substring_terms: usize,
    pub substring_grams: usize,
    pub substring_lookup_requests: usize,
    pub substring_lookup_ids: usize,
    pub substring_candidate_ids: usize,
    pub substring_cache_hits: usize,
    pub substring_cache_misses: usize,
    pub substring_cutoff_terms: usize,
    pub substring_term_truncated_grams: usize,
    pub substring_truncated_grams: usize,
    pub fuzzy_terms: usize,
    pub fuzzy_keys: usize,
    pub fuzzy_lookup_requests: usize,
    pub fuzzy_lookup_terms: usize,
    pub fuzzy_candidate_terms: usize,
    pub fuzzy_verified_candidates: usize,
    pub fuzzy_cache_hits: usize,
    pub fuzzy_cache_misses: usize,
    pub fuzzy_key_truncated_terms: usize,
    pub fuzzy_term_truncated_keys: usize,
    pub fuzzy_candidate_truncated_terms: usize,
}

impl SearchLookupTelemetry {
    pub fn merge(&mut self, other: &Self) {
        self.prefix_terms += other.prefix_terms;
        self.prefix_lookup_requests += other.prefix_lookup_requests;
        self.prefix_lookup_ids += other.prefix_lookup_ids;
        self.prefix_candidate_ids += other.prefix_candidate_ids;
        self.prefix_cache_hits += other.prefix_cache_hits;
        self.prefix_cache_misses += other.prefix_cache_misses;
        self.prefix_cutoff_terms += other.prefix_cutoff_terms;
        self.prefix_truncated_terms += other.prefix_truncated_terms;
        self.substring_terms += other.substring_terms;
        self.substring_grams += other.substring_grams;
        self.substring_lookup_requests += other.substring_lookup_requests;
        self.substring_lookup_ids += other.substring_lookup_ids;
        self.substring_candidate_ids += other.substring_candidate_ids;
        self.substring_cache_hits += other.substring_cache_hits;
        self.substring_cache_misses += other.substring_cache_misses;
        self.substring_cutoff_terms += other.substring_cutoff_terms;
        self.substring_term_truncated_grams += other.substring_term_truncated_grams;
        self.substring_truncated_grams += other.substring_truncated_grams;
        self.fuzzy_terms += other.fuzzy_terms;
        self.fuzzy_keys += other.fuzzy_keys;
        self.fuzzy_lookup_requests += other.fuzzy_lookup_requests;
        self.fuzzy_lookup_terms += other.fuzzy_lookup_terms;
        self.fuzzy_candidate_terms += other.fuzzy_candidate_terms;
        self.fuzzy_verified_candidates += other.fuzzy_verified_candidates;
        self.fuzzy_cache_hits += other.fuzzy_cache_hits;
        self.fuzzy_cache_misses += other.fuzzy_cache_misses;
        self.fuzzy_key_truncated_terms += other.fuzzy_key_truncated_terms;
        self.fuzzy_term_truncated_keys += other.fuzzy_term_truncated_keys;
        self.fuzzy_candidate_truncated_terms += other.fuzzy_candidate_truncated_terms;
    }

    pub fn merge_cache_delta(&mut self, before: &Self, after: &Self) {
        self.prefix_lookup_requests += after
            .prefix_lookup_requests
            .saturating_sub(before.prefix_lookup_requests);
        self.prefix_cache_hits += after
            .prefix_cache_hits
            .saturating_sub(before.prefix_cache_hits);
        self.prefix_cache_misses += after
            .prefix_cache_misses
            .saturating_sub(before.prefix_cache_misses);
        self.substring_lookup_requests += after
            .substring_lookup_requests
            .saturating_sub(before.substring_lookup_requests);
        self.substring_cache_hits += after
            .substring_cache_hits
            .saturating_sub(before.substring_cache_hits);
        self.substring_cache_misses += after
            .substring_cache_misses
            .saturating_sub(before.substring_cache_misses);
        self.fuzzy_lookup_requests += after
            .fuzzy_lookup_requests
            .saturating_sub(before.fuzzy_lookup_requests);
        self.fuzzy_cache_hits += after
            .fuzzy_cache_hits
            .saturating_sub(before.fuzzy_cache_hits);
        self.fuzzy_cache_misses += after
            .fuzzy_cache_misses
            .saturating_sub(before.fuzzy_cache_misses);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQueryReport {
    pub hits: Vec<SearchHit>,
    pub lookup: SearchLookupTelemetry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchStreamStage {
    Hot,
    Deep,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchStreamBatch {
    pub stage: SearchStreamStage,
    pub hits: Vec<SearchHit>,
}

pub fn substring_candidate_grams(query: &str) -> Vec<String> {
    let parsed = SearchQuery::parse(query);
    substring_grams(&parsed.terms.join(" "))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchFuzzyPosting {
    pub key: String,
    pub terms: Vec<String>,
}

pub trait SearchLookup: Sync {
    fn prefix_ids(&self, prefix: &str) -> gfm_types::Result<Vec<FileId>>;
    fn substring_ids(&self, gram: &str) -> gfm_types::Result<Vec<FileId>>;
    fn fuzzy_terms(&self, key: &str) -> gfm_types::Result<Vec<String>>;

    fn prefix_ids_bounded(&self, prefix: &str, limit: usize) -> gfm_types::Result<SearchLookupIds> {
        let mut ids = self.prefix_ids(prefix)?;
        let truncated = ids.len() > limit;
        ids.truncate(limit);
        Ok(SearchLookupIds::new(ids, truncated))
    }

    fn substring_ids_bounded(
        &self,
        gram: &str,
        limit: usize,
    ) -> gfm_types::Result<SearchLookupIds> {
        let mut ids = self.substring_ids(gram)?;
        let truncated = ids.len() > limit;
        ids.truncate(limit);
        Ok(SearchLookupIds::new(ids, truncated))
    }

    fn fuzzy_terms_bounded(&self, key: &str, limit: usize) -> gfm_types::Result<SearchLookupTerms> {
        let mut terms = self.fuzzy_terms(key)?;
        let truncated = terms.len() > limit;
        terms.truncate(limit);
        Ok(SearchLookupTerms::new(terms, truncated))
    }

    fn cache_telemetry(&self) -> SearchLookupTelemetry {
        SearchLookupTelemetry::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchLookupIds {
    pub ids: Vec<FileId>,
    pub truncated: bool,
}

impl SearchLookupIds {
    pub fn new(ids: Vec<FileId>, truncated: bool) -> Self {
        Self { ids, truncated }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchLookupTerms {
    pub terms: Vec<String>,
    pub truncated: bool,
}

impl SearchLookupTerms {
    pub fn new(terms: Vec<String>, truncated: bool) -> Self {
        Self { terms, truncated }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EmptySearchLookup;

impl SearchLookup for EmptySearchLookup {
    fn prefix_ids(&self, _prefix: &str) -> gfm_types::Result<Vec<FileId>> {
        Ok(Vec::new())
    }

    fn substring_ids(&self, _gram: &str) -> gfm_types::Result<Vec<FileId>> {
        Ok(Vec::new())
    }

    fn fuzzy_terms(&self, _key: &str) -> gfm_types::Result<Vec<String>> {
        Ok(Vec::new())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPrefixPosting {
    pub prefix: String,
    pub ids: Vec<FileId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchSubstringPosting {
    pub gram: String,
    pub ids: Vec<FileId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMetadataField {
    Tag,
    Comment,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMetadataPosting {
    pub field: SearchMetadataField,
    pub term: String,
    pub ids: Vec<FileId>,
}

#[derive(Debug, Clone, Default)]
pub struct SearchIndex {
    records: HashMap<FileId, FileRecord>,
    columns: HashMap<FileId, RecordColumns>,
    paths: HashMap<String, FileId>,
    name_exact: BTreeMap<String, BTreeSet<FileId>>,
    name_prefixes: BTreeMap<String, BTreeSet<FileId>>,
    name_substrings: BTreeMap<String, BTreeSet<FileId>>,
    name_terms: BTreeMap<String, BTreeSet<FileId>>,
    path_terms: BTreeMap<String, BTreeSet<FileId>>,
    metadata_terms: BTreeMap<String, BTreeSet<FileId>>,
    fuzzy_terms: BTreeMap<String, BTreeSet<String>>,
    extension: BTreeMap<String, BTreeSet<FileId>>,
    tags: BTreeMap<String, BTreeSet<FileId>>,
    kind: HashMap<FileKind, BTreeSet<FileId>>,
    content_terms: BTreeMap<String, BTreeMap<FileId, Vec<u32>>>,
    content_record_terms: HashMap<FileId, BTreeSet<String>>,
    pinned: BTreeSet<FileId>,
}

impl SearchIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn indexed_name_prefixes(&self) -> usize {
        self.name_prefixes.len()
    }

    pub fn insert(&mut self, record: FileRecord) {
        let id = record.id;
        if let Some(old) = self.records.remove(&id) {
            self.remove_terms(&old);
            self.columns.remove(&id);
            self.paths.remove(&path_key(&old.path));
        }
        let columns = RecordColumns::from_record(&record);
        self.add_terms(&record, &columns);
        self.paths.insert(path_key(&record.path), id);
        self.columns.insert(id, columns);
        self.records.insert(id, record);
    }

    pub fn insert_with_columns(
        &mut self,
        record: FileRecord,
        columns: SearchRecordColumns,
    ) -> bool {
        self.insert_with_columns_inner(record, columns, true, true, true, true)
    }

    pub fn insert_with_columns_deferred_fuzzy(
        &mut self,
        record: FileRecord,
        columns: SearchRecordColumns,
    ) -> bool {
        self.insert_with_columns_inner(record, columns, true, true, false, true)
    }

    pub fn insert_with_columns_deferred_sidecars(
        &mut self,
        record: FileRecord,
        columns: SearchRecordColumns,
    ) -> bool {
        self.insert_with_columns_inner(record, columns, false, false, false, false)
    }

    fn insert_with_columns_inner(
        &mut self,
        record: FileRecord,
        columns: SearchRecordColumns,
        build_prefixes: bool,
        build_substrings: bool,
        build_fuzzy: bool,
        build_metadata: bool,
    ) -> bool {
        if record.id != columns.id {
            self.insert(record);
            return false;
        }
        let id = record.id;
        if let Some(old) = self.records.remove(&id) {
            self.remove_terms(&old);
            self.columns.remove(&id);
            self.paths.remove(&path_key(&old.path));
        }
        let normalized = RecordColumns::from_search_columns(&columns);
        self.add_terms_with_sidecar_policy(
            &record,
            &normalized,
            build_prefixes,
            build_substrings,
            build_fuzzy,
            build_metadata,
        );
        self.paths.insert(path_key(&record.path), id);
        self.columns.insert(id, normalized);
        self.records.insert(id, record);
        true
    }

    pub fn import_prefix_postings(&mut self, postings: &[SearchPrefixPosting]) -> usize {
        for posting in postings {
            let prefix = normalize(&posting.prefix);
            if !is_prefix_term(&prefix) {
                continue;
            }
            let ids = posting
                .ids
                .iter()
                .copied()
                .filter(|id| self.records.contains_key(id))
                .collect::<BTreeSet<_>>();
            if !ids.is_empty() {
                self.name_prefixes.entry(prefix).or_default().extend(ids);
            }
        }
        self.name_prefixes.len()
    }

    pub fn import_substring_postings(&mut self, postings: &[SearchSubstringPosting]) -> usize {
        for posting in postings {
            let gram = normalize(&posting.gram);
            if !is_substring_gram(&gram) {
                continue;
            }
            let ids = posting
                .ids
                .iter()
                .copied()
                .filter(|id| self.records.contains_key(id))
                .collect::<BTreeSet<_>>();
            if !ids.is_empty() {
                self.name_substrings.entry(gram).or_default().extend(ids);
            }
        }
        self.name_substrings.len()
    }

    pub fn import_fuzzy_postings(&mut self, postings: &[SearchFuzzyPosting]) -> usize {
        for posting in postings {
            let key = normalize(&posting.key);
            if key.is_empty() {
                continue;
            }
            let terms = posting
                .terms
                .iter()
                .map(|term| normalize(term))
                .filter(|term| is_fuzzy_term(term))
                .collect::<BTreeSet<_>>();
            if !terms.is_empty() {
                self.fuzzy_terms.entry(key).or_default().extend(terms);
            }
        }
        self.fuzzy_terms.len()
    }

    pub fn import_metadata_postings(&mut self, postings: &[SearchMetadataPosting]) -> usize {
        for posting in postings {
            let term = normalize(&posting.term);
            if term.is_empty() {
                continue;
            }
            let ids = posting
                .ids
                .iter()
                .copied()
                .filter(|id| self.records.contains_key(id))
                .collect::<BTreeSet<_>>();
            if ids.is_empty() {
                continue;
            }
            match posting.field {
                SearchMetadataField::Tag => self.tags.entry(term).or_default().extend(ids),
                SearchMetadataField::Comment => {
                    self.metadata_terms.entry(term).or_default().extend(ids)
                }
            }
        }
        self.tags.len() + self.metadata_terms.len()
    }

    pub fn apply_record_columns(&mut self, columns: SearchRecordColumns) -> bool {
        let Some(record) = self.records.get(&columns.id).cloned() else {
            return false;
        };
        self.remove_terms(&record);
        let normalized = RecordColumns::from_search_columns(&columns);
        self.add_terms(&record, &normalized);
        self.columns.insert(columns.id, normalized);
        true
    }

    pub fn remove(&mut self, id: FileId) -> Option<FileRecord> {
        let record = self.records.remove(&id)?;
        self.remove_terms(&record);
        self.columns.remove(&id);
        self.pinned.remove(&id);
        self.paths.remove(&path_key(&record.path));
        Some(record)
    }

    pub fn remove_path(&mut self, path: impl AsRef<std::path::Path>) -> Option<FileRecord> {
        let id = self.paths.remove(&path_key(path.as_ref()))?;
        self.remove(id)
    }

    pub fn remove_subtree(&mut self, root: impl AsRef<std::path::Path>) -> Vec<FileRecord> {
        let root = path_key(root.as_ref());
        let prefix = format!("{root}/");
        let ids: Vec<_> = self
            .paths
            .iter()
            .filter_map(|(path, id)| (path == &root || path.starts_with(&prefix)).then_some(*id))
            .collect();

        let mut removed = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(record) = self.remove(id) {
                removed.push(record);
            }
        }
        removed
    }

    pub fn get_path(&self, path: impl AsRef<std::path::Path>) -> Option<&FileRecord> {
        let id = self.paths.get(&path_key(path.as_ref()))?;
        self.records.get(id)
    }

    pub fn records(&self) -> impl Iterator<Item = &FileRecord> {
        self.records.values()
    }

    pub fn pin(&mut self, id: FileId) -> bool {
        if self.records.contains_key(&id) {
            self.pinned.insert(id)
        } else {
            false
        }
    }

    pub fn unpin(&mut self, id: FileId) -> bool {
        self.pinned.remove(&id)
    }

    pub fn is_pinned(&self, id: FileId) -> bool {
        self.pinned.contains(&id)
    }

    #[cfg(test)]
    fn name_prefix_posting_count(&self, prefix: &str) -> usize {
        self.name_prefixes
            .get(prefix)
            .map(BTreeSet::len)
            .unwrap_or(0)
    }

    #[cfg(test)]
    fn content_record_term_count(&self, id: FileId) -> usize {
        self.content_record_terms
            .get(&id)
            .map(BTreeSet::len)
            .unwrap_or(0)
    }

    pub fn insert_content(&mut self, id: FileId, text: &str) {
        if !self.records.contains_key(&id) {
            return;
        }
        self.remove_content(id);
        for (position, token) in tokenize(&normalize(text)).into_iter().enumerate() {
            self.content_record_terms
                .entry(id)
                .or_default()
                .insert(token.clone());
            self.content_terms
                .entry(token)
                .or_default()
                .entry(id)
                .or_default()
                .push(position as u32);
        }
    }

    pub fn insert_content_terms(&mut self, id: FileId, terms: impl IntoIterator<Item = String>) {
        if !self.records.contains_key(&id) {
            return;
        }
        for term in terms {
            let term = normalize(&term);
            if !term.is_empty() {
                self.content_record_terms
                    .entry(id)
                    .or_default()
                    .insert(term.clone());
                self.content_terms
                    .entry(term)
                    .or_default()
                    .entry(id)
                    .or_default();
            }
        }
    }

    pub fn import_content_postings(&mut self, postings: &[ContentPosting]) {
        for posting in postings {
            let term = normalize(&posting.term);
            if term.is_empty() {
                continue;
            }
            for id in &posting.ids {
                if self.records.contains_key(id) {
                    self.content_record_terms
                        .entry(*id)
                        .or_default()
                        .insert(term.clone());
                    self.content_terms
                        .entry(term.clone())
                        .or_default()
                        .entry(*id)
                        .or_default();
                }
            }
            for positions in &posting.positions {
                if self.records.contains_key(&positions.id) {
                    let mut normalized_positions = positions.positions.clone();
                    normalized_positions.sort_unstable();
                    normalized_positions.dedup();
                    self.content_record_terms
                        .entry(positions.id)
                        .or_default()
                        .insert(term.clone());
                    self.content_terms
                        .entry(term.clone())
                        .or_default()
                        .insert(positions.id, normalized_positions);
                }
            }
        }
    }

    pub fn remove_content(&mut self, id: FileId) {
        let Some(terms) = self.content_record_terms.remove(&id) else {
            for positions in self.content_terms.values_mut() {
                positions.remove(&id);
            }
            self.content_terms
                .retain(|_, positions| !positions.is_empty());
            return;
        };
        for term in terms {
            if let Some(positions) = self.content_terms.get_mut(&term) {
                positions.remove(&id);
                if positions.is_empty() {
                    self.content_terms.remove(&term);
                }
            }
        }
    }

    pub fn content_postings(&self) -> Vec<ContentPosting> {
        self.content_terms
            .iter()
            .map(|(term, positions)| ContentPosting {
                term: term.clone(),
                ids: positions.keys().copied().collect(),
                positions: positions
                    .iter()
                    .filter(|(_, positions)| !positions.is_empty())
                    .map(|(id, positions)| ContentPositions {
                        id: *id,
                        positions: positions.clone(),
                    })
                    .collect(),
            })
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
    ) -> gfm_types::Result<Vec<SearchHit>> {
        self.query_structured_cancellable(&SearchQuery::parse(query), limit, cancellation)
    }

    pub fn query_structured_cancellable(
        &self,
        query: &SearchQuery,
        limit: usize,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<Vec<SearchHit>> {
        self.query_structured_with_lookup_cancellable(
            query,
            limit,
            &EmptySearchLookup,
            cancellation,
        )
    }

    pub fn query_structured_with_lookup_cancellable(
        &self,
        query: &SearchQuery,
        limit: usize,
        lookup: &dyn SearchLookup,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<Vec<SearchHit>> {
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
    ) -> gfm_types::Result<SearchQueryReport> {
        self.query_pass(query, limit, SearchPass::Full, lookup, budget, cancellation)
    }

    pub fn stream(&self, query: &str, limit: usize) -> gfm_types::Result<Vec<SearchStreamBatch>> {
        self.stream_structured(&SearchQuery::parse(query), limit)
    }

    pub fn stream_structured(
        &self,
        query: &SearchQuery,
        limit: usize,
    ) -> gfm_types::Result<Vec<SearchStreamBatch>> {
        self.stream_structured_cancellable(query, limit, &Cancellation::default())
    }

    pub fn stream_cancellable(
        &self,
        query: &str,
        limit: usize,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<Vec<SearchStreamBatch>> {
        self.stream_structured_cancellable(&SearchQuery::parse(query), limit, cancellation)
    }

    pub fn stream_structured_cancellable(
        &self,
        query: &SearchQuery,
        limit: usize,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<Vec<SearchStreamBatch>> {
        cancellation.check()?;
        if query.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let hot = self
            .query_pass(
                query,
                limit,
                SearchPass::Hot,
                &EmptySearchLookup,
                SearchLookupBudget::default(),
                cancellation,
            )?
            .hits;
        let full = self
            .query_pass(
                query,
                limit,
                SearchPass::Full,
                &EmptySearchLookup,
                SearchLookupBudget::default(),
                cancellation,
            )?
            .hits;
        let mut seen: HashMap<FileId, i64> = HashMap::new();
        let mut batches = Vec::new();

        if !hot.is_empty() {
            for hit in &hot {
                seen.insert(hit.record.id, hit.score);
            }
            batches.push(SearchStreamBatch {
                stage: SearchStreamStage::Hot,
                hits: hot,
            });
        }

        let deep: Vec<_> = full
            .into_iter()
            .filter(|hit| match seen.get(&hit.record.id) {
                Some(score) => hit.score > *score,
                None => true,
            })
            .collect();
        if !deep.is_empty() {
            batches.push(SearchStreamBatch {
                stage: SearchStreamStage::Deep,
                hits: deep,
            });
        }
        cancellation.check()?;
        Ok(batches)
    }

    fn query_pass(
        &self,
        query: &SearchQuery,
        limit: usize,
        pass: SearchPass,
        lookup: &dyn SearchLookup,
        budget: SearchLookupBudget,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<SearchQueryReport> {
        cancellation.check()?;
        if query.is_empty() || limit == 0 {
            return Ok(SearchQueryReport {
                hits: Vec::new(),
                lookup: SearchLookupTelemetry::default(),
            });
        }

        let mut scores: HashMap<FileId, RankAccumulator> = HashMap::new();
        let mut telemetry = SearchLookupTelemetry::default();
        let text = query.terms.join(" ");
        let intent = QueryIntent::from_query(query);
        if let Some(report) =
            self.query_simple_single_term_pass(query, limit, pass, lookup, budget, cancellation)?
        {
            return Ok(report);
        }
        if let Some(report) =
            self.query_simple_multi_term_pass(query, limit, pass, lookup, budget, cancellation)?
        {
            return Ok(report);
        }
        let expression_candidates = query
            .expression
            .as_ref()
            .and_then(|expression| self.expression_candidate_ids(expression, pass));

        if !text.is_empty() {
            if let Some(ids) = self.name_exact.get(&text) {
                add_scores(&mut scores, ids, EXACT_NAME, MatchReason::ExactName);
            }
        }

        if !text.is_empty() {
            let ids = self.name_prefix_ids(&text, lookup, budget, &mut telemetry)?;
            if !ids.is_empty() {
                add_scores(&mut scores, &ids, PREFIX_NAME, MatchReason::PrefixName);
            }
        }

        for term in &query.terms {
            cancellation.check()?;
            if let Some(ids) = self.name_terms.get(term) {
                add_scores(&mut scores, ids, NAME_TOKEN, MatchReason::SubstringName);
            }
            if let Some(ids) = self.path_terms.get(term) {
                add_scores(&mut scores, ids, PATH_COMPONENT, MatchReason::PathComponent);
            }
            if let Some(ids) = self.metadata_terms.get(term) {
                add_scores(&mut scores, ids, TAG, MatchReason::Tag);
            }
            if let Some(ids) = self.extension.get(term) {
                add_scores(&mut scores, ids, EXTENSION, MatchReason::Extension);
            }
            if let Some(ids) = self.tags.get(term) {
                add_scores(&mut scores, ids, TAG, MatchReason::Tag);
            }
            if pass.includes_deep() {
                self.add_content_scores(&mut scores, term);
            }
        }

        if pass.includes_deep() {
            for term in &query.terms {
                cancellation.check()?;
                for id in self.fuzzy_ids(term, lookup, budget, &mut telemetry)? {
                    cancellation.check()?;
                    let Some(record) = self.records.get(&id) else {
                        continue;
                    };
                    if !self.record_contains_term(record, term)
                        && self.record_fuzzy_matches_term(record, term)
                    {
                        scores
                            .entry(id)
                            .and_modify(|score| score.add(FUZZY_NAME, MatchReason::FuzzyName))
                            .or_insert_with(|| {
                                RankAccumulator::new(FUZZY_NAME, MatchReason::FuzzyName)
                            });
                    }
                }
            }
        }

        for phrase in &query.phrases {
            cancellation.check()?;
            let mut phrase_matches = HashMap::new();
            for id in self.record_phrase_ids(phrase) {
                phrase_matches.insert(id, MatchReason::PathComponent);
            }
            if pass.includes_deep() {
                for id in self.content_phrase_ids(phrase) {
                    phrase_matches.insert(id, MatchReason::Content);
                }
            }
            for (id, reason) in phrase_matches {
                cancellation.check()?;
                scores
                    .entry(id)
                    .and_modify(|score| score.add(PHRASE, reason.clone()))
                    .or_insert_with(|| RankAccumulator::new(PHRASE, reason));
            }
        }

        if pass.includes_deep() {
            for proximity in &query.proximities {
                cancellation.check()?;
                for id in self.content_proximity_ids(proximity) {
                    scores
                        .entry(id)
                        .and_modify(|score| score.add(PROXIMITY, MatchReason::Content))
                        .or_insert_with(|| RankAccumulator::new(PROXIMITY, MatchReason::Content));
                }
            }
        }

        if scores.len() < limit {
            let candidates = self.name_substring_ids(&text, lookup, budget, &mut telemetry)?;
            for id in candidates {
                cancellation.check()?;
                let Some(record) = self.records.get(&id) else {
                    continue;
                };
                if !text.is_empty()
                    && self
                        .columns
                        .get(&record.id)
                        .is_some_and(|columns| columns.name.contains(&text))
                {
                    scores
                        .entry(record.id)
                        .and_modify(|score| score.add(SUBSTRING_NAME, MatchReason::SubstringName))
                        .or_insert_with(|| {
                            RankAccumulator::new(SUBSTRING_NAME, MatchReason::SubstringName)
                        });
                }
            }
        }

        if let Some(ids) = &expression_candidates {
            seed_scores(&mut scores, ids);
        }

        if !intent.is_empty() {
            for record in self.records.values() {
                cancellation.check()?;
                let score = intent.score(record);
                if score > 0 {
                    scores
                        .entry(record.id)
                        .and_modify(|current| current.boost(score))
                        .or_insert_with(|| RankAccumulator::new(score, MatchReason::PathComponent));
                }
            }
        }

        if query.expression.as_ref().is_some_and(|expression| {
            expression_needs_universe(expression) && expression_candidates.is_none()
        }) || (expression_candidates.is_none()
            && scores.is_empty()
            && !query.filters.is_empty()
            && query.terms.is_empty()
            && query.phrases.is_empty()
            && query.proximities.is_empty())
        {
            for record in self.records.values() {
                cancellation.check()?;
                scores
                    .entry(record.id)
                    .or_insert_with(|| RankAccumulator::new(0, MatchReason::PathComponent));
            }
        }

        for (id, score) in &mut scores {
            let Some(record) = self.records.get(id) else {
                continue;
            };
            score.boost(self.composite_boosts(record, query, pass));
        }

        let mut hits = BoundedHitMerge::new(limit);
        for (id, score) in scores {
            cancellation.check()?;
            let Some(record) = self.records.get(&id) else {
                continue;
            };
            if !self.record_matches_query(record, query, pass) {
                continue;
            }
            let (score, reason) = score.finish();
            hits.push(SearchHit {
                record: record.clone(),
                score,
                reason,
                snippet: None,
            });
        }

        let hits = hits.into_sorted_hits();
        cancellation.check()?;
        Ok(SearchQueryReport {
            hits,
            lookup: telemetry,
        })
    }

    fn query_simple_single_term_pass(
        &self,
        query: &SearchQuery,
        limit: usize,
        pass: SearchPass,
        lookup: &dyn SearchLookup,
        budget: SearchLookupBudget,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<Option<SearchQueryReport>> {
        if pass != SearchPass::Full
            || query.expression.is_some()
            || query.terms.len() != 1
            || !query.excluded_terms.is_empty()
            || !query.phrases.is_empty()
            || !query.proximities.is_empty()
            || !query.filters.is_empty()
            || !QueryIntent::from_query(query).is_empty()
        {
            return Ok(None);
        }

        let term = &query.terms[0];
        let mut scores: HashMap<FileId, RankAccumulator> = HashMap::new();
        let mut telemetry = SearchLookupTelemetry::default();
        if let Some(ids) = self.name_exact.get(term) {
            add_scores(&mut scores, ids, EXACT_NAME, MatchReason::ExactName);
        }
        let ids = self.name_prefix_ids(term, lookup, budget, &mut telemetry)?;
        if !ids.is_empty() {
            add_scores(&mut scores, &ids, PREFIX_NAME, MatchReason::PrefixName);
        }
        for postings in [
            self.name_terms
                .get(term)
                .map(|ids| (ids, NAME_TOKEN, MatchReason::SubstringName)),
            self.path_terms
                .get(term)
                .map(|ids| (ids, PATH_COMPONENT, MatchReason::PathComponent)),
            self.metadata_terms
                .get(term)
                .map(|ids| (ids, TAG, MatchReason::Tag)),
            self.extension
                .get(term)
                .map(|ids| (ids, EXTENSION, MatchReason::Extension)),
            self.tags.get(term).map(|ids| (ids, TAG, MatchReason::Tag)),
        ]
        .into_iter()
        .flatten()
        {
            add_scores(&mut scores, postings.0, postings.1, postings.2);
        }
        for id in self.fuzzy_ids(term, lookup, budget, &mut telemetry)? {
            cancellation.check()?;
            let Some(record) = self.records.get(&id) else {
                continue;
            };
            if !self.record_contains_term(record, term)
                && self.record_fuzzy_matches_term(record, term)
            {
                scores
                    .entry(id)
                    .and_modify(|score| score.add(FUZZY_NAME, MatchReason::FuzzyName))
                    .or_insert_with(|| RankAccumulator::new(FUZZY_NAME, MatchReason::FuzzyName));
            }
        }

        let mut hits = BoundedHitMerge::new(limit);
        if let Some(content_positions) = self.content_terms.get(term) {
            for id in content_positions.keys() {
                cancellation.check()?;
                if let Some(score) = scores.get_mut(id) {
                    score.add(CONTENT, MatchReason::Content);
                    continue;
                }
                let Some(record) = self.records.get(id) else {
                    continue;
                };
                let mut score = RankAccumulator::new(CONTENT, MatchReason::Content);
                score.boost(self.composite_boosts(record, query, pass));
                let (score, reason) = score.finish();
                hits.push(SearchHit {
                    record: record.clone(),
                    score,
                    reason,
                    snippet: None,
                });
            }
        }

        for (id, mut score) in scores {
            cancellation.check()?;
            let Some(record) = self.records.get(&id) else {
                continue;
            };
            if !self.record_matches_query(record, query, pass) {
                continue;
            }
            score.boost(self.composite_boosts(record, query, pass));
            let (score, reason) = score.finish();
            hits.push(SearchHit {
                record: record.clone(),
                score,
                reason,
                snippet: None,
            });
        }

        Ok(Some(SearchQueryReport {
            hits: hits.into_sorted_hits(),
            lookup: telemetry,
        }))
    }

    fn query_simple_multi_term_pass(
        &self,
        query: &SearchQuery,
        limit: usize,
        pass: SearchPass,
        lookup: &dyn SearchLookup,
        budget: SearchLookupBudget,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<Option<SearchQueryReport>> {
        if pass != SearchPass::Full
            || query.expression.is_some()
            || query.terms.len() < 2
            || !query.excluded_terms.is_empty()
            || !query.phrases.is_empty()
            || !query.proximities.is_empty()
            || !query.filters.is_empty()
            || !QueryIntent::from_query(query).is_empty()
        {
            return Ok(None);
        }

        let text = query.terms.join(" ");
        let mut telemetry = SearchLookupTelemetry::default();
        let full_text_prefix_ids = self.name_prefix_ids(&text, lookup, budget, &mut telemetry)?;
        let mut fuzzy_by_term = Vec::with_capacity(query.terms.len());
        let mut candidate_sets = Vec::with_capacity(query.terms.len());
        for term in &query.terms {
            cancellation.check()?;
            let fuzzy_ids = self.fuzzy_ids(term, lookup, budget, &mut telemetry)?;
            let mut candidates = self.term_candidate_ids(term, SearchPass::Full);
            candidates.extend(fuzzy_ids.iter().copied());
            if candidates.is_empty() {
                return Ok(Some(SearchQueryReport {
                    hits: Vec::new(),
                    lookup: telemetry,
                }));
            }
            fuzzy_by_term.push(fuzzy_ids);
            candidate_sets.push(candidates);
        }

        let Some(anchor) = candidate_sets.iter().min_by_key(|ids| ids.len()) else {
            return Ok(None);
        };
        let mut hits = BoundedHitMerge::new(limit);
        for id in anchor {
            cancellation.check()?;
            let Some(record) = self.records.get(id) else {
                continue;
            };
            if !self.record_matches_query(record, query, pass) {
                continue;
            }
            let mut score = self.score_plain_multi_term_record(
                record,
                query,
                &text,
                &full_text_prefix_ids,
                &fuzzy_by_term,
            );
            score.boost(self.composite_boosts(record, query, pass));
            let (score, reason) = score.finish();
            hits.push(SearchHit {
                record: record.clone(),
                score,
                reason,
                snippet: None,
            });
        }

        Ok(Some(SearchQueryReport {
            hits: hits.into_sorted_hits(),
            lookup: telemetry,
        }))
    }

    fn score_plain_multi_term_record(
        &self,
        record: &FileRecord,
        query: &SearchQuery,
        text: &str,
        full_text_prefix_ids: &BTreeSet<FileId>,
        fuzzy_by_term: &[BTreeSet<FileId>],
    ) -> RankAccumulator {
        let mut score = RankAccumulator::new(0, MatchReason::PathComponent);
        if self
            .name_exact
            .get(text)
            .is_some_and(|ids| ids.contains(&record.id))
        {
            score.add(EXACT_NAME, MatchReason::ExactName);
        }
        if full_text_prefix_ids.contains(&record.id) {
            score.add(PREFIX_NAME, MatchReason::PrefixName);
        }
        for (index, term) in query.terms.iter().enumerate() {
            if self
                .name_terms
                .get(term)
                .is_some_and(|ids| ids.contains(&record.id))
            {
                score.add(NAME_TOKEN, MatchReason::SubstringName);
            }
            if self
                .path_terms
                .get(term)
                .is_some_and(|ids| ids.contains(&record.id))
            {
                score.add(PATH_COMPONENT, MatchReason::PathComponent);
            }
            if self
                .metadata_terms
                .get(term)
                .is_some_and(|ids| ids.contains(&record.id))
            {
                score.add(TAG, MatchReason::Tag);
            }
            if self
                .extension
                .get(term)
                .is_some_and(|ids| ids.contains(&record.id))
            {
                score.add(EXTENSION, MatchReason::Extension);
            }
            if self
                .tags
                .get(term)
                .is_some_and(|ids| ids.contains(&record.id))
            {
                score.add(TAG, MatchReason::Tag);
            }
            if self.content_has(record.id, term) {
                score.add(CONTENT, MatchReason::Content);
            }
            if fuzzy_by_term
                .get(index)
                .is_some_and(|ids| ids.contains(&record.id))
                && !self.record_contains_term(record, term)
                && self.record_fuzzy_matches_term(record, term)
            {
                score.add(FUZZY_NAME, MatchReason::FuzzyName);
            }
        }
        score
    }

    fn record_matches_query(
        &self,
        record: &FileRecord,
        query: &SearchQuery,
        pass: SearchPass,
    ) -> bool {
        if let Some(expression) = &query.expression {
            return self.record_matches_expression(record, expression, pass);
        }
        if query.excluded_terms.iter().any(|term| {
            self.record_contains_term(record, term)
                || (pass.includes_deep() && self.content_has(record.id, term))
        }) {
            return false;
        }
        if !query.phrases.iter().all(|phrase| {
            self.record_matches_phrase(record, phrase)
                || (pass.includes_deep() && self.content_matches_phrase(record.id, phrase))
        }) {
            return false;
        }
        if pass.includes_deep()
            && !query
                .proximities
                .iter()
                .all(|proximity| self.content_matches_proximity(record.id, proximity))
        {
            return false;
        }
        if !pass.includes_deep() && !query.proximities.is_empty() {
            return false;
        }
        query
            .filters
            .iter()
            .all(|filter| self.filter_matches(record, filter))
    }

    fn record_matches_expression(
        &self,
        record: &FileRecord,
        expression: &QueryExpr,
        pass: SearchPass,
    ) -> bool {
        match expression {
            QueryExpr::Term(term) => {
                self.record_contains_term(record, term)
                    || (pass.includes_deep() && self.content_has(record.id, term))
                    || (pass.includes_deep() && self.record_fuzzy_matches_term(record, term))
                    || term_matches_intent(term, record)
            }
            QueryExpr::Phrase(phrase) => {
                self.record_matches_phrase(record, phrase)
                    || (pass.includes_deep() && self.content_matches_phrase(record.id, phrase))
            }
            QueryExpr::Proximity(proximity) => {
                pass.includes_deep() && self.content_matches_proximity(record.id, proximity)
            }
            QueryExpr::Filter(filter) => self.filter_matches(record, filter),
            QueryExpr::Not(expression) => !self.record_matches_expression(record, expression, pass),
            QueryExpr::And(expressions) => expressions
                .iter()
                .all(|expression| self.record_matches_expression(record, expression, pass)),
            QueryExpr::Or(expressions) => expressions
                .iter()
                .any(|expression| self.record_matches_expression(record, expression, pass)),
        }
    }

    fn composite_boosts(&self, record: &FileRecord, query: &SearchQuery, pass: SearchPass) -> i64 {
        let mut score = recency_score(record);
        let Some(columns) = self.columns.get(&record.id) else {
            return score;
        };
        if self.pinned.contains(&record.id) {
            score += USER_PINNED;
        }
        for filter in &query.filters {
            if filter_kind_matches(filter, record.kind) {
                score += kind_score(record.kind);
            } else if filter_matches_columns(filter, record, columns) {
                score += KIND_MATCH / 3;
            }
        }
        for term in &query.terms {
            score += capped_frequency(count_term(&columns.name, term), NAME_FREQUENCY);
            score += capped_frequency(count_term(&columns.path, term), PATH_FREQUENCY);
            if pass.includes_deep() {
                score +=
                    capped_frequency(self.content_frequency(record.id, term), CONTENT_FREQUENCY);
            }
        }
        score
    }

    fn content_has(&self, id: FileId, term: &str) -> bool {
        self.content_terms
            .get(term)
            .is_some_and(|positions| positions.contains_key(&id))
    }

    fn add_content_scores(&self, scores: &mut HashMap<FileId, RankAccumulator>, term: &str) {
        let Some(positions) = self.content_terms.get(term) else {
            return;
        };
        for id in positions.keys() {
            scores
                .entry(*id)
                .and_modify(|score| score.add(CONTENT, MatchReason::Content))
                .or_insert_with(|| RankAccumulator::new(CONTENT, MatchReason::Content));
        }
    }

    fn content_frequency(&self, id: FileId, term: &str) -> usize {
        self.content_terms
            .get(term)
            .and_then(|positions| positions.get(&id))
            .map(|positions| positions.len().max(1))
            .unwrap_or(0)
    }

    fn fuzzy_ids(
        &self,
        term: &str,
        lookup: &dyn SearchLookup,
        budget: SearchLookupBudget,
        telemetry: &mut SearchLookupTelemetry,
    ) -> gfm_types::Result<BTreeSet<FileId>> {
        if !is_fuzzy_term(term) {
            return Ok(BTreeSet::new());
        }
        telemetry.fuzzy_terms += 1;
        let mut ids = BTreeSet::new();
        let keys = deletion_keys(term, 2);
        if keys.len() > budget.max_fuzzy_keys_per_term {
            telemetry.fuzzy_term_truncated_keys += 1;
        }
        for key in keys.into_iter().take(budget.max_fuzzy_keys_per_term) {
            telemetry.fuzzy_keys += 1;
            let mut candidates = self.fuzzy_terms.get(&key).cloned().unwrap_or_default();
            let mut candidate_truncated = false;
            if candidates.len() > budget.max_fuzzy_candidates_per_term {
                candidate_truncated = true;
                candidates = candidates
                    .into_iter()
                    .take(budget.max_fuzzy_candidates_per_term)
                    .collect();
            }
            let remaining_candidates = budget
                .max_fuzzy_candidates_per_term
                .saturating_sub(candidates.len());
            if remaining_candidates > 0 {
                let lookup_limit = budget.max_fuzzy_terms_per_key.min(remaining_candidates);
                let lookup_terms = lookup.fuzzy_terms_bounded(&key, lookup_limit)?;
                telemetry.fuzzy_lookup_terms += lookup_terms.terms.len();
                if lookup_terms.truncated {
                    telemetry.fuzzy_key_truncated_terms += 1;
                }
                candidates.extend(
                    lookup_terms
                        .terms
                        .into_iter()
                        .map(|term| normalize(&term))
                        .filter(|term| is_fuzzy_term(term)),
                );
            }
            telemetry.fuzzy_candidate_terms += candidates.len();
            if candidate_truncated {
                telemetry.fuzzy_candidate_truncated_terms += 1;
            }
            for candidate in candidates {
                telemetry.fuzzy_verified_candidates += 1;
                if bounded_levenshtein(&candidate, term, 2).is_some() {
                    if let Some(matches) = self.name_terms.get(&candidate) {
                        ids.extend(matches);
                    }
                }
            }
        }
        Ok(ids)
    }

    fn name_prefix_ids(
        &self,
        term: &str,
        lookup: &dyn SearchLookup,
        budget: SearchLookupBudget,
        telemetry: &mut SearchLookupTelemetry,
    ) -> gfm_types::Result<BTreeSet<FileId>> {
        if !is_prefix_term(term) {
            return Ok(BTreeSet::new());
        }
        telemetry.prefix_terms += 1;
        let mut ids = self.name_prefixes.get(term).cloned().unwrap_or_default();
        if ids.len() > budget.max_prefix_ids_per_term {
            telemetry.prefix_truncated_terms += 1;
            ids = ids
                .into_iter()
                .take(budget.max_prefix_ids_per_term)
                .collect();
        }
        let remaining = budget.max_prefix_ids_per_term.saturating_sub(ids.len());
        if remaining == 0 || term.chars().count() < budget.min_archive_prefix_chars {
            telemetry.prefix_cutoff_terms += 1;
            telemetry.prefix_candidate_ids += ids.len();
            return Ok(ids);
        }
        let lookup_ids = lookup.prefix_ids_bounded(term, remaining)?;
        telemetry.prefix_lookup_ids += lookup_ids.ids.len();
        if lookup_ids.truncated {
            telemetry.prefix_truncated_terms += 1;
        }
        ids.extend(
            lookup_ids
                .ids
                .into_iter()
                .filter(|id| self.records.contains_key(id)),
        );
        telemetry.prefix_candidate_ids += ids.len();
        Ok(ids)
    }

    fn name_substring_ids(
        &self,
        term: &str,
        lookup: &dyn SearchLookup,
        budget: SearchLookupBudget,
        telemetry: &mut SearchLookupTelemetry,
    ) -> gfm_types::Result<BTreeSet<FileId>> {
        if term.is_empty() {
            return Ok(BTreeSet::new());
        }
        if term.chars().count() < SUBSTRING_GRAM_CHARS {
            telemetry.substring_cutoff_terms += 1;
            return Ok(BTreeSet::new());
        }
        let grams = substring_grams(term);
        if grams.is_empty() {
            telemetry.substring_cutoff_terms += 1;
            return Ok(BTreeSet::new());
        }
        telemetry.substring_terms += 1;
        if grams.len() > budget.max_substring_grams_per_term {
            telemetry.substring_term_truncated_grams += 1;
        }

        let mut gram_sets = Vec::new();
        for gram in grams.into_iter().take(budget.max_substring_grams_per_term) {
            telemetry.substring_grams += 1;
            let mut ids = self.name_substrings.get(&gram).cloned().unwrap_or_default();
            let mut gram_truncated = false;
            if ids.len() > budget.max_substring_ids_per_gram {
                gram_truncated = true;
                ids = ids
                    .into_iter()
                    .take(budget.max_substring_ids_per_gram)
                    .collect();
            }
            let remaining = budget.max_substring_ids_per_gram.saturating_sub(ids.len());
            if remaining > 0 {
                let lookup_ids = lookup.substring_ids_bounded(&gram, remaining)?;
                telemetry.substring_lookup_ids += lookup_ids.ids.len();
                if lookup_ids.truncated {
                    gram_truncated = true;
                }
                ids.extend(
                    lookup_ids
                        .ids
                        .into_iter()
                        .filter(|id| self.records.contains_key(id)),
                );
            }
            if gram_truncated {
                telemetry.substring_truncated_grams += 1;
            }
            if ids.is_empty() {
                return Ok(BTreeSet::new());
            }
            gram_sets.push(ids);
        }

        gram_sets.sort_by_key(|ids| ids.len());
        let mut gram_sets = gram_sets.into_iter();
        let mut candidates = gram_sets.next().unwrap_or_default();
        for ids in gram_sets {
            candidates.retain(|id| ids.contains(id));
            if candidates.is_empty() {
                break;
            }
        }
        telemetry.substring_candidate_ids += candidates.len();
        Ok(candidates)
    }

    fn content_matches_phrase(&self, id: FileId, phrase: &str) -> bool {
        let terms = tokenize(&normalize(phrase));
        if terms.is_empty() {
            return false;
        }
        if terms.len() == 1 {
            return self.content_has(id, &terms[0]);
        }

        let mut positions = Vec::with_capacity(terms.len());
        for term in &terms {
            let Some(term_positions) = self
                .content_terms
                .get(term)
                .and_then(|positions| positions.get(&id))
                .filter(|positions| !positions.is_empty())
            else {
                return false;
            };
            positions.push(term_positions);
        }

        let Some((anchor_offset, anchor_positions)) = positions
            .iter()
            .enumerate()
            .min_by_key(|(_, positions)| positions.len())
        else {
            return false;
        };
        anchor_positions.iter().copied().any(|anchor| {
            let Some(start) = anchor.checked_sub(anchor_offset as u32) else {
                return false;
            };
            positions
                .iter()
                .enumerate()
                .all(|(offset, term_positions)| {
                    start
                        .checked_add(offset as u32)
                        .is_some_and(|position| sorted_contains_position(term_positions, position))
                })
        })
    }

    fn record_phrase_ids(&self, phrase: &str) -> Vec<FileId> {
        let terms = tokenize(&normalize(phrase));
        if terms.is_empty() {
            return self
                .records
                .values()
                .filter(|record| self.record_matches_phrase(record, phrase))
                .map(|record| record.id)
                .collect();
        }

        let mut ids = BTreeSet::new();
        self.add_record_phrase_ids_for_field(
            &terms,
            phrase,
            &self.name_terms,
            |columns, phrase| columns.matches_name_phrase(phrase),
            &mut ids,
        );
        self.add_record_phrase_ids_for_field(
            &terms,
            phrase,
            &self.path_terms,
            |columns, phrase| columns.matches_path_phrase(phrase),
            &mut ids,
        );
        self.add_record_phrase_ids_for_field(
            &terms,
            phrase,
            &self.metadata_terms,
            |columns, phrase| columns.matches_comment_phrase(phrase),
            &mut ids,
        );
        ids.into_iter().collect()
    }

    fn add_record_phrase_ids_for_field(
        &self,
        terms: &[String],
        phrase: &str,
        postings: &BTreeMap<String, BTreeSet<FileId>>,
        matches: impl Fn(&RecordColumns, &str) -> bool,
        ids: &mut BTreeSet<FileId>,
    ) {
        let Some(candidates) = rarest_term_postings(terms, postings) else {
            return;
        };
        ids.extend(candidates.iter().copied().filter(|id| {
            self.columns
                .get(id)
                .is_some_and(|columns| matches(columns, phrase))
        }));
    }

    fn content_phrase_ids(&self, phrase: &str) -> Vec<FileId> {
        let terms = tokenize(&normalize(phrase));
        let Some(candidates) = rarest_content_postings(&terms, &self.content_terms) else {
            return Vec::new();
        };
        candidates
            .keys()
            .copied()
            .filter(|id| self.content_matches_phrase(*id, phrase))
            .collect()
    }

    fn content_proximity_ids(&self, proximity: &QueryProximity) -> Vec<FileId> {
        let postings: Option<Vec<_>> = proximity
            .terms
            .iter()
            .map(|term| self.content_terms.get(term))
            .collect();
        let Some(mut postings) = postings else {
            return Vec::new();
        };
        let Some((rarest_index, _)) = postings
            .iter()
            .enumerate()
            .min_by_key(|(_, positions)| positions.len())
        else {
            return Vec::new();
        };
        let rarest = postings.swap_remove(rarest_index);
        rarest
            .keys()
            .copied()
            .filter(|id| postings.iter().all(|positions| positions.contains_key(id)))
            .filter(|id| self.content_matches_proximity(*id, proximity))
            .collect()
    }

    fn content_matches_proximity(&self, id: FileId, proximity: &QueryProximity) -> bool {
        let mut positions = Vec::with_capacity(proximity.terms.len());
        for term in &proximity.terms {
            let Some(term_positions) = self
                .content_terms
                .get(term)
                .and_then(|positions| positions.get(&id))
                .filter(|positions| !positions.is_empty())
            else {
                return false;
            };
            positions.push(term_positions);
        }

        let Some((anchor_index, anchor_positions)) = positions
            .iter()
            .enumerate()
            .min_by_key(|(_, positions)| positions.len())
        else {
            return false;
        };
        anchor_positions.iter().copied().any(|anchor| {
            positions.iter().enumerate().all(|(index, other)| {
                index == anchor_index
                    || sorted_has_position_within(other, anchor, proximity.distance)
            })
        })
    }

    fn add_terms(&mut self, record: &FileRecord, columns: &RecordColumns) {
        self.add_terms_with_sidecar_policy(record, columns, true, true, true, true);
    }

    fn add_terms_with_sidecar_policy(
        &mut self,
        record: &FileRecord,
        columns: &RecordColumns,
        build_prefixes: bool,
        build_substrings: bool,
        build_fuzzy: bool,
        build_metadata: bool,
    ) {
        self.name_exact
            .entry(columns.name.clone())
            .or_default()
            .insert(record.id);
        for token in &columns.name_tokens {
            let is_new = !self.name_terms.contains_key(token);
            self.name_terms
                .entry(token.clone())
                .or_default()
                .insert(record.id);
            if build_prefixes {
                for prefix in token_prefixes(token) {
                    self.name_prefixes
                        .entry(prefix)
                        .or_default()
                        .insert(record.id);
                }
            }
            if build_fuzzy && is_new {
                self.add_fuzzy_term(token);
            }
        }
        if build_substrings {
            for gram in substring_grams(&columns.name) {
                self.name_substrings
                    .entry(gram)
                    .or_default()
                    .insert(record.id);
            }
        }
        for token in &columns.path_tokens {
            self.path_terms
                .entry(token.clone())
                .or_default()
                .insert(record.id);
        }
        if let Some(ext) = &columns.extension {
            self.extension
                .entry(ext.clone())
                .or_default()
                .insert(record.id);
        }
        self.kind.entry(record.kind).or_default().insert(record.id);
        if build_metadata {
            for tag in &columns.tags {
                self.tags.entry(tag.clone()).or_default().insert(record.id);
            }
            for token in &columns.metadata_tokens {
                self.metadata_terms
                    .entry(token.clone())
                    .or_default()
                    .insert(record.id);
            }
        }
    }

    fn remove_terms(&mut self, record: &FileRecord) {
        let columns = self
            .columns
            .get(&record.id)
            .cloned()
            .unwrap_or_else(|| RecordColumns::from_record(record));
        remove_id(&mut self.name_exact, &columns.name, record.id);
        for gram in substring_grams(&columns.name) {
            remove_id(&mut self.name_substrings, &gram, record.id);
        }
        for token in &columns.name_tokens {
            remove_id(&mut self.name_terms, token, record.id);
            for prefix in token_prefixes(token) {
                remove_id(&mut self.name_prefixes, &prefix, record.id);
            }
            if !self.name_terms.contains_key(token) {
                self.remove_fuzzy_term(token);
            }
        }
        for token in &columns.path_tokens {
            remove_id(&mut self.path_terms, token, record.id);
        }
        if let Some(ext) = &columns.extension {
            remove_id(&mut self.extension, ext, record.id);
        }
        if let Some(ids) = self.kind.get_mut(&record.kind) {
            ids.remove(&record.id);
            if ids.is_empty() {
                self.kind.remove(&record.kind);
            }
        }
        for tag in &columns.tags {
            remove_id(&mut self.tags, tag, record.id);
        }
        for token in &columns.metadata_tokens {
            remove_id(&mut self.metadata_terms, token, record.id);
        }
        self.remove_content(record.id);
    }

    fn add_fuzzy_term(&mut self, term: &str) {
        if !is_fuzzy_term(term) {
            return;
        }
        for key in deletion_keys(term, 2) {
            self.fuzzy_terms
                .entry(key)
                .or_default()
                .insert(term.to_string());
        }
    }

    fn remove_fuzzy_term(&mut self, term: &str) {
        if !is_fuzzy_term(term) {
            return;
        }
        for key in deletion_keys(term, 2) {
            if let Some(terms) = self.fuzzy_terms.get_mut(&key) {
                terms.remove(term);
                if terms.is_empty() {
                    self.fuzzy_terms.remove(&key);
                }
            }
        }
    }

    fn record_contains_term(&self, record: &FileRecord, term: &str) -> bool {
        self.columns
            .get(&record.id)
            .is_some_and(|columns| columns.contains_term(term))
    }

    fn record_matches_phrase(&self, record: &FileRecord, phrase: &str) -> bool {
        self.columns
            .get(&record.id)
            .is_some_and(|columns| columns.matches_phrase(phrase))
    }

    fn filter_matches(&self, record: &FileRecord, filter: &QueryFilter) -> bool {
        self.columns
            .get(&record.id)
            .is_some_and(|columns| filter_matches_columns(filter, record, columns))
    }

    fn expression_candidate_ids(
        &self,
        expression: &QueryExpr,
        pass: SearchPass,
    ) -> Option<BTreeSet<FileId>> {
        match expression {
            QueryExpr::Term(term) => Some(self.term_candidate_ids(term, pass)),
            QueryExpr::Phrase(phrase) => Some(
                self.record_phrase_ids(phrase)
                    .into_iter()
                    .chain(
                        pass.includes_deep()
                            .then(|| self.content_phrase_ids(phrase))
                            .into_iter()
                            .flatten(),
                    )
                    .collect(),
            ),
            QueryExpr::Proximity(proximity) => pass
                .includes_deep()
                .then(|| self.content_proximity_ids(proximity).into_iter().collect()),
            QueryExpr::Filter(filter) => self.filter_candidate_ids(filter),
            QueryExpr::Not(_) => None,
            QueryExpr::And(expressions) => self.and_expression_candidate_ids(expressions, pass),
            QueryExpr::Or(expressions) => {
                if expressions.is_empty() {
                    return Some(BTreeSet::new());
                }
                let mut ids = BTreeSet::new();
                for expression in expressions {
                    ids.extend(self.expression_candidate_ids(expression, pass)?);
                }
                Some(ids)
            }
        }
    }

    fn and_expression_candidate_ids(
        &self,
        expressions: &[QueryExpr],
        pass: SearchPass,
    ) -> Option<BTreeSet<FileId>> {
        let exact = expressions
            .iter()
            .filter_map(|expression| self.exact_expression_candidate_ids(expression, pass));

        if let Some(ids) = intersect_candidate_sets(exact) {
            return Some(ids);
        }

        expressions
            .iter()
            .filter_map(|expression| self.expression_candidate_ids(expression, pass))
            .min_by_key(BTreeSet::len)
    }

    fn exact_expression_candidate_ids(
        &self,
        expression: &QueryExpr,
        pass: SearchPass,
    ) -> Option<BTreeSet<FileId>> {
        match expression {
            QueryExpr::Phrase(phrase) => Some(
                self.record_phrase_ids(phrase)
                    .into_iter()
                    .chain(
                        pass.includes_deep()
                            .then(|| self.content_phrase_ids(phrase))
                            .into_iter()
                            .flatten(),
                    )
                    .collect(),
            ),
            QueryExpr::Proximity(proximity) => pass
                .includes_deep()
                .then(|| self.content_proximity_ids(proximity).into_iter().collect()),
            QueryExpr::Filter(filter) => self.filter_candidate_ids(filter),
            QueryExpr::And(expressions) => {
                self.exact_and_expression_candidate_ids(expressions, pass)
            }
            QueryExpr::Or(expressions) => {
                if expressions.is_empty() {
                    return Some(BTreeSet::new());
                }
                let mut ids = BTreeSet::new();
                for expression in expressions {
                    ids.extend(self.exact_expression_candidate_ids(expression, pass)?);
                }
                Some(ids)
            }
            QueryExpr::Term(_) | QueryExpr::Not(_) => None,
        }
    }

    fn exact_and_expression_candidate_ids(
        &self,
        expressions: &[QueryExpr],
        pass: SearchPass,
    ) -> Option<BTreeSet<FileId>> {
        intersect_candidate_sets(
            expressions
                .iter()
                .map(|expression| self.exact_expression_candidate_ids(expression, pass))
                .collect::<Option<Vec<_>>>()?,
        )
        .or_else(|| Some(BTreeSet::new()))
    }

    fn term_candidate_ids(&self, term: &str, pass: SearchPass) -> BTreeSet<FileId> {
        let mut ids = BTreeSet::new();
        for postings in [
            self.name_terms.get(term),
            self.path_terms.get(term),
            self.metadata_terms.get(term),
            self.extension.get(term),
            self.tags.get(term),
        ]
        .into_iter()
        .flatten()
        {
            ids.extend(postings);
        }
        if pass.includes_deep() {
            if let Some(postings) = self.content_terms.get(term) {
                ids.extend(postings.keys());
            }
        }
        ids
    }

    fn filter_candidate_ids(&self, filter: &QueryFilter) -> Option<BTreeSet<FileId>> {
        match filter {
            QueryFilter::Name(value, false) => {
                self.text_filter_candidate_ids(value, &self.name_terms, |columns, value| {
                    columns.name.contains(value)
                })
            }
            QueryFilter::Path(value, false) => {
                self.text_filter_candidate_ids(value, &self.path_terms, |columns, value| {
                    columns.path.contains(value)
                })
            }
            QueryFilter::Extension(value, false) => {
                Some(self.extension.get(value).cloned().unwrap_or_default())
            }
            QueryFilter::Tag(value, false) => {
                Some(self.tags.get(value).cloned().unwrap_or_default())
            }
            QueryFilter::Kind(kind, false) => Some(
                self.kind
                    .get(&query_kind_file_kind(*kind))
                    .cloned()
                    .unwrap_or_default(),
            ),
            QueryFilter::Scope(_, false)
            | QueryFilter::Size(_, false)
            | QueryFilter::Date(_, _, false) => None,
            QueryFilter::Name(_, true)
            | QueryFilter::Path(_, true)
            | QueryFilter::Extension(_, true)
            | QueryFilter::Tag(_, true)
            | QueryFilter::Scope(_, true)
            | QueryFilter::Kind(_, true)
            | QueryFilter::Size(_, true)
            | QueryFilter::Date(_, _, true) => None,
        }
    }

    fn text_filter_candidate_ids(
        &self,
        value: &str,
        postings: &BTreeMap<String, BTreeSet<FileId>>,
        matches: impl Fn(&RecordColumns, &str) -> bool,
    ) -> Option<BTreeSet<FileId>> {
        let terms = tokenize(value);
        let candidates = rarest_term_postings(&terms, postings)?;
        Some(
            candidates
                .iter()
                .copied()
                .filter(|id| {
                    self.columns
                        .get(id)
                        .is_some_and(|columns| matches(columns, value))
                })
                .collect(),
        )
    }

    fn record_fuzzy_matches_term(&self, record: &FileRecord, term: &str) -> bool {
        self.columns.get(&record.id).is_some_and(|columns| {
            columns
                .fuzzy_terms
                .iter()
                .any(|candidate| bounded_levenshtein(candidate, term, 2).is_some())
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchPass {
    Hot,
    Full,
}

impl SearchPass {
    fn includes_deep(self) -> bool {
        self == Self::Full
    }
}

fn path_key(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

fn is_fuzzy_term(term: &str) -> bool {
    let mut count = 0;
    let mut has_alpha = false;
    let mut consecutive_digits = 0;
    for ch in term.chars() {
        count += 1;
        if ch.is_alphabetic() {
            has_alpha = true;
            consecutive_digits = 0;
        } else if ch.is_ascii_digit() {
            consecutive_digits += 1;
            if consecutive_digits > 4 {
                return false;
            }
        } else {
            consecutive_digits = 0;
        }
    }
    (FUZZY_MIN_TERM_LEN..=FUZZY_MAX_TERM_LEN).contains(&count) && has_alpha
}

fn is_prefix_term(term: &str) -> bool {
    (PREFIX_MIN_TERM_LEN..=PREFIX_MAX_TERM_LEN).contains(&term.chars().count())
}

fn token_prefixes(term: &str) -> impl Iterator<Item = String> + '_ {
    term.char_indices()
        .map(|(index, _)| index)
        .skip(1)
        .chain(std::iter::once(term.len()))
        .take(PREFIX_MAX_TERM_LEN)
        .map(|end| term[..end].to_string())
}

fn substring_grams(value: &str) -> Vec<String> {
    let mut starts = value
        .char_indices()
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    starts.push(value.len());
    if starts.len() <= SUBSTRING_GRAM_CHARS {
        return Vec::new();
    }
    let mut grams = starts
        .windows(SUBSTRING_GRAM_CHARS + 1)
        .map(|window| value[window[0]..window[SUBSTRING_GRAM_CHARS]].to_string())
        .collect::<Vec<_>>();
    grams.sort();
    grams.dedup();
    grams
}

fn is_substring_gram(value: &str) -> bool {
    value.chars().count() == SUBSTRING_GRAM_CHARS
}

fn expression_needs_universe(expression: &QueryExpr) -> bool {
    match expression {
        QueryExpr::Filter(_) | QueryExpr::Not(_) => true,
        QueryExpr::And(expressions) => {
            !expression_has_positive_anchor(expression)
                || expressions.iter().all(expression_needs_universe)
        }
        QueryExpr::Or(expressions) => expressions.iter().any(expression_needs_universe),
        QueryExpr::Term(_) | QueryExpr::Phrase(_) | QueryExpr::Proximity(_) => false,
    }
}

fn expression_has_positive_anchor(expression: &QueryExpr) -> bool {
    match expression {
        QueryExpr::Term(_) | QueryExpr::Phrase(_) | QueryExpr::Proximity(_) => true,
        QueryExpr::Filter(_) | QueryExpr::Not(_) => false,
        QueryExpr::And(expressions) => expressions.iter().any(expression_has_positive_anchor),
        QueryExpr::Or(expressions) => {
            !expressions.is_empty() && expressions.iter().all(expression_has_positive_anchor)
        }
    }
}

fn rarest_term_postings<'a>(
    terms: &[String],
    postings: &'a BTreeMap<String, BTreeSet<FileId>>,
) -> Option<&'a BTreeSet<FileId>> {
    terms
        .iter()
        .map(|term| postings.get(term))
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .min_by_key(|ids| ids.len())
}

fn rarest_content_postings<'a>(
    terms: &[String],
    postings: &'a BTreeMap<String, BTreeMap<FileId, Vec<u32>>>,
) -> Option<&'a BTreeMap<FileId, Vec<u32>>> {
    terms
        .iter()
        .map(|term| postings.get(term))
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .min_by_key(|ids| ids.len())
}

fn intersect_candidate_sets<I>(candidate_sets: I) -> Option<BTreeSet<FileId>>
where
    I: IntoIterator<Item = BTreeSet<FileId>>,
{
    let mut candidate_sets = candidate_sets.into_iter().collect::<Vec<_>>();
    candidate_sets.sort_by_key(BTreeSet::len);

    let mut sets = candidate_sets.into_iter();
    let mut ids = sets.next()?;
    for candidates in sets {
        ids.retain(|id| candidates.contains(id));
        if ids.is_empty() {
            break;
        }
    }
    Some(ids)
}

fn sorted_contains_position(positions: &[u32], position: u32) -> bool {
    debug_assert!(positions.windows(2).all(|window| window[0] < window[1]));
    positions.binary_search(&position).is_ok()
}

fn sorted_has_position_within(positions: &[u32], anchor: u32, distance: u32) -> bool {
    debug_assert!(positions.windows(2).all(|window| window[0] < window[1]));
    let min = anchor.saturating_sub(distance);
    let max = anchor.saturating_add(distance);
    let index = positions.partition_point(|position| *position < min);
    positions
        .get(index)
        .is_some_and(|position| *position <= max)
}

fn add_scores(
    scores: &mut HashMap<FileId, RankAccumulator>,
    ids: &BTreeSet<FileId>,
    points: i64,
    reason: MatchReason,
) {
    for id in ids {
        scores
            .entry(*id)
            .and_modify(|score| score.add(points, reason.clone()))
            .or_insert_with(|| RankAccumulator::new(points, reason.clone()));
    }
}

fn seed_scores(scores: &mut HashMap<FileId, RankAccumulator>, ids: &BTreeSet<FileId>) {
    for id in ids {
        scores
            .entry(*id)
            .or_insert_with(|| RankAccumulator::new(0, MatchReason::PathComponent));
    }
}

fn query_kind_file_kind(kind: QueryKind) -> FileKind {
    match kind {
        QueryKind::Directory => FileKind::Directory,
        QueryKind::File => FileKind::File,
        QueryKind::Symlink => FileKind::Symlink,
        QueryKind::Other => FileKind::Other,
    }
}

fn remove_id(map: &mut BTreeMap<String, BTreeSet<FileId>>, key: &str, id: FileId) {
    if let Some(ids) = map.get_mut(key) {
        ids.remove(&id);
        if ids.is_empty() {
            map.remove(key);
        }
    }
}

pub(crate) fn sort_hits(hits: &mut [SearchHit]) {
    hits.sort_by_cached_key(|hit| {
        (
            Reverse(hit.score),
            hit.record.name.to_lowercase(),
            hit.record.path.to_string_lossy().into_owned(),
            hit.record.id,
        )
    });
}

#[derive(Debug)]
pub(crate) struct BoundedHitMerge {
    limit: usize,
    hits: Vec<SearchHit>,
}

impl BoundedHitMerge {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            limit,
            hits: Vec::with_capacity(limit),
        }
    }

    pub(crate) fn push(&mut self, hit: SearchHit) {
        if self.limit == 0 {
            return;
        }
        self.hits.push(hit);
        if self.hits.len() > self.limit.saturating_mul(2) {
            self.trim();
        }
    }

    pub(crate) fn extend(&mut self, hits: Vec<SearchHit>) {
        if self.limit == 0 || hits.is_empty() {
            return;
        }
        self.hits.extend(hits);
        if self.hits.len() > self.limit.saturating_mul(2) {
            self.trim();
        }
    }

    pub(crate) fn into_sorted_hits(mut self) -> Vec<SearchHit> {
        self.trim();
        self.hits
    }

    fn trim(&mut self) {
        sort_hits(&mut self.hits);
        self.hits.truncate(self.limit);
    }
}

pub(crate) fn top_hits(hits: Vec<SearchHit>, limit: usize) -> Vec<SearchHit> {
    let mut merge = BoundedHitMerge::new(limit);
    merge.extend(hits);
    merge.into_sorted_hits()
}

#[cfg(test)]
mod tests;
