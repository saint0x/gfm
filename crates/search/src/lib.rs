mod candidates;
mod columns;
mod content;
mod fuzzy;
mod hits;
mod ingest;
mod intent;
mod lookup;
mod matchers;
mod pass;
mod query;
mod ranking;
mod scoring;
mod session;
mod shard;
mod simple;
mod terms;

#[cfg(test)]
use candidates::expression_has_positive_anchor;
use candidates::expression_needs_universe;
#[cfg(test)]
use candidates::intersect_candidate_sets;
use columns::RecordColumns;
pub use columns::SearchRecordColumns;
#[cfg(test)]
use content::sorted_has_position_within;
pub(crate) use fuzzy::deletion_keys;
use hits::BoundedHitMerge;
use intent::QueryIntent;
pub use lookup::{
    EmptySearchLookup, SearchLookup, SearchLookupBudget, SearchLookupIds, SearchLookupTelemetry,
    SearchLookupTerms,
};
pub(crate) use pass::{rarest_term_postings, SearchPass};
use query::{normalize, tokenize};
pub use query::{
    DateComparison, DateField, QueryExpr, QueryFilter, QueryKind, QueryProximity, QueryScope,
    SearchQuery, SizeComparison,
};
use ranking::{
    RankAccumulator, CONTENT, EXACT_NAME, EXTENSION, FUZZY_NAME, NAME_TOKEN, PATH_COMPONENT,
    PHRASE, PREFIX_NAME, PROXIMITY, SUBSTRING_NAME, TAG,
};
use scoring::{add_scores_cancellable, seed_scores_cancellable};
pub use session::SearchSupersession;
pub use shard::{SearchVolumeScope, ShardedSearchIndex};
use terms::substring_grams;
pub(crate) use terms::{is_fuzzy_term, is_prefix_term};

use gfm_jobs::Cancellation;
use gfm_types::{FileId, FileKind, FileRecord, MatchReason, SearchHit};
use std::collections::{BTreeMap, BTreeSet, HashMap};

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
            .map(|expression| {
                self.expression_candidate_ids_cancellable(expression, pass, cancellation)
            })
            .transpose()?
            .flatten();

        if !text.is_empty() {
            if let Some(ids) = self.name_exact.get(&text) {
                add_scores_cancellable(
                    &mut scores,
                    ids,
                    EXACT_NAME,
                    MatchReason::ExactName,
                    cancellation,
                )?;
            }
        }

        if !text.is_empty() {
            let ids = self.name_prefix_ids(&text, lookup, budget, &mut telemetry, cancellation)?;
            if !ids.is_empty() {
                add_scores_cancellable(
                    &mut scores,
                    &ids,
                    PREFIX_NAME,
                    MatchReason::PrefixName,
                    cancellation,
                )?;
            }
        }

        for term in &query.terms {
            cancellation.check()?;
            if let Some(ids) = self.name_terms.get(term) {
                add_scores_cancellable(
                    &mut scores,
                    ids,
                    NAME_TOKEN,
                    MatchReason::SubstringName,
                    cancellation,
                )?;
            }
            if let Some(ids) = self.path_terms.get(term) {
                add_scores_cancellable(
                    &mut scores,
                    ids,
                    PATH_COMPONENT,
                    MatchReason::PathComponent,
                    cancellation,
                )?;
            }
            if let Some(ids) = self.metadata_terms.get(term) {
                add_scores_cancellable(&mut scores, ids, TAG, MatchReason::Tag, cancellation)?;
            }
            if let Some(ids) = self.extension.get(term) {
                add_scores_cancellable(
                    &mut scores,
                    ids,
                    EXTENSION,
                    MatchReason::Extension,
                    cancellation,
                )?;
            }
            if let Some(ids) = self.tags.get(term) {
                add_scores_cancellable(&mut scores, ids, TAG, MatchReason::Tag, cancellation)?;
            }
            if pass.includes_deep() {
                self.add_content_scores(&mut scores, term, cancellation)?;
            }
        }

        if pass.includes_deep() {
            for term in &query.terms {
                cancellation.check()?;
                for id in self.fuzzy_ids(term, lookup, budget, &mut telemetry, cancellation)? {
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
            let candidates =
                self.name_substring_ids(&text, lookup, budget, &mut telemetry, cancellation)?;
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
            seed_scores_cancellable(&mut scores, ids, cancellation)?;
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
}

#[cfg(test)]
pub(crate) use hits::sort_hits;
pub(crate) use hits::top_hits;

#[cfg(test)]
mod tests;
