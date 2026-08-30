use super::columns::filter_matches_columns;
use super::ranking::{
    capped_frequency, count_term, filter_kind_matches, kind_score, recency_score, RankAccumulator,
    CONTENT, CONTENT_FREQUENCY, EXACT_NAME, EXTENSION, FUZZY_NAME, KIND_MATCH, NAME_FREQUENCY,
    NAME_TOKEN, PATH_COMPONENT, PATH_FREQUENCY, PREFIX_NAME, TAG, USER_PINNED,
};
use super::{SearchIndex, SearchPass, SearchQuery};
use gfm_jobs::Cancellation;
use gfm_types::{FileId, FileRecord, MatchReason};
use std::collections::{BTreeSet, HashMap};

const CANCELLATION_STRIDE: usize = 256;

impl SearchIndex {
    pub(super) fn score_plain_multi_term_record(
        &self,
        record: &FileRecord,
        query: &SearchQuery,
        text: &str,
        full_text_prefix_ids: &BTreeSet<FileId>,
        fuzzy_by_term: &[BTreeSet<FileId>],
        pass: SearchPass,
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
            if pass.includes_deep() && self.content_has(record.id, term) {
                score.add(CONTENT, MatchReason::Content);
            }
            if pass.includes_deep()
                && fuzzy_by_term
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

    pub(super) fn composite_boosts(
        &self,
        record: &FileRecord,
        query: &SearchQuery,
        pass: SearchPass,
    ) -> i64 {
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
}

pub(super) fn add_scores_cancellable(
    scores: &mut HashMap<FileId, RankAccumulator>,
    ids: &BTreeSet<FileId>,
    points: i64,
    reason: MatchReason,
    cancellation: &Cancellation,
) -> gfm_types::Result<()> {
    cancellation.check()?;
    for (index, id) in ids.iter().enumerate() {
        if index % CANCELLATION_STRIDE == 0 {
            cancellation.check()?;
        }
        scores
            .entry(*id)
            .and_modify(|score| score.add(points, reason.clone()))
            .or_insert_with(|| RankAccumulator::new(points, reason.clone()));
    }
    Ok(())
}

pub(super) fn seed_scores_cancellable(
    scores: &mut HashMap<FileId, RankAccumulator>,
    ids: &BTreeSet<FileId>,
    cancellation: &Cancellation,
) -> gfm_types::Result<()> {
    cancellation.check()?;
    for (index, id) in ids.iter().enumerate() {
        if index % CANCELLATION_STRIDE == 0 {
            cancellation.check()?;
        }
        scores
            .entry(*id)
            .or_insert_with(|| RankAccumulator::new(0, MatchReason::PathComponent));
    }
    Ok(())
}
