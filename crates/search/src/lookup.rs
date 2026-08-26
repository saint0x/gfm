use super::fuzzy::{bounded_levenshtein, deletion_keys};
use super::query::normalize;
use super::terms::{is_fuzzy_term, is_prefix_term, substring_grams, SUBSTRING_GRAM_CHARS};
use super::SearchIndex;
use gfm_types::{FileId, VolumeId};
use std::collections::BTreeSet;

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

    fn prefix_ids_for_volume(
        &self,
        prefix: &str,
        volume: VolumeId,
    ) -> gfm_types::Result<Vec<FileId>> {
        Ok(self
            .prefix_ids(prefix)?
            .into_iter()
            .filter(|id| id.volume == volume)
            .collect())
    }

    fn substring_ids_for_volume(
        &self,
        gram: &str,
        volume: VolumeId,
    ) -> gfm_types::Result<Vec<FileId>> {
        Ok(self
            .substring_ids(gram)?
            .into_iter()
            .filter(|id| id.volume == volume)
            .collect())
    }

    fn prefix_ids_for_volume_bounded(
        &self,
        prefix: &str,
        volume: VolumeId,
        limit: usize,
    ) -> gfm_types::Result<SearchLookupIds> {
        let mut ids = self.prefix_ids_for_volume(prefix, volume)?;
        let truncated = ids.len() > limit;
        ids.truncate(limit);
        Ok(SearchLookupIds::new(ids, truncated))
    }

    fn substring_ids_for_volume_bounded(
        &self,
        gram: &str,
        volume: VolumeId,
        limit: usize,
    ) -> gfm_types::Result<SearchLookupIds> {
        let mut ids = self.substring_ids_for_volume(gram, volume)?;
        let truncated = ids.len() > limit;
        ids.truncate(limit);
        Ok(SearchLookupIds::new(ids, truncated))
    }

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

impl SearchIndex {
    pub(super) fn fuzzy_ids(
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

    pub(super) fn name_prefix_ids(
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

    pub(super) fn name_substring_ids(
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
}
