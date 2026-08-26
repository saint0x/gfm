use gfm_types::FileId;

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
