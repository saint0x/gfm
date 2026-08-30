use crate::hits::BoundedHitMerge;
use crate::lookup::{SearchLookup, SearchLookupBudget, SearchLookupTelemetry};
use crate::pass::SearchPass;
use crate::ranking::{
    RankAccumulator, CONTENT, EXACT_NAME, EXTENSION, FUZZY_NAME, NAME_TOKEN, PATH_COMPONENT,
    PREFIX_NAME, TAG,
};
use crate::scoring::add_scores_cancellable;
use crate::{QueryIntent, SearchIndex, SearchQuery, SearchQueryReport};
use gfm_jobs::Cancellation;
use gfm_types::{FileId, MatchReason, SearchHit};
use std::collections::{BTreeSet, HashMap};

impl SearchIndex {
    pub(super) fn query_simple_single_term_pass(
        &self,
        query: &SearchQuery,
        limit: usize,
        pass: SearchPass,
        lookup: &dyn SearchLookup,
        budget: SearchLookupBudget,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<Option<SearchQueryReport>> {
        if query.expression.is_some()
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
            add_scores_cancellable(
                &mut scores,
                ids,
                EXACT_NAME,
                MatchReason::ExactName,
                cancellation,
            )?;
        }
        let ids = self.name_prefix_ids(term, lookup, budget, &mut telemetry, cancellation)?;
        if !ids.is_empty() {
            add_scores_cancellable(
                &mut scores,
                &ids,
                PREFIX_NAME,
                MatchReason::PrefixName,
                cancellation,
            )?;
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
            add_scores_cancellable(
                &mut scores,
                postings.0,
                postings.1,
                postings.2,
                cancellation,
            )?;
        }
        if pass.includes_deep() {
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

        let mut hits = BoundedHitMerge::new(limit);
        if pass.includes_deep() {
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

    pub(super) fn query_simple_multi_term_pass(
        &self,
        query: &SearchQuery,
        limit: usize,
        pass: SearchPass,
        lookup: &dyn SearchLookup,
        budget: SearchLookupBudget,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<Option<SearchQueryReport>> {
        if query.expression.is_some()
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
        let full_text_prefix_ids =
            self.name_prefix_ids(&text, lookup, budget, &mut telemetry, cancellation)?;
        let mut fuzzy_by_term = Vec::with_capacity(query.terms.len());
        let mut candidate_sets = Vec::with_capacity(query.terms.len());
        for term in &query.terms {
            cancellation.check()?;
            let fuzzy_ids = if pass.includes_deep() {
                self.fuzzy_ids(term, lookup, budget, &mut telemetry, cancellation)?
            } else {
                BTreeSet::new()
            };
            let mut candidates = self.term_candidate_ids_cancellable(term, pass, cancellation)?;
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
                pass,
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
}
