use super::{
    normalize, rarest_term_postings, tokenize, QueryProximity, RankAccumulator, RecordColumns,
    SearchIndex, CONTENT,
};
use gfm_jobs::Cancellation;
use gfm_types::{FileId, MatchReason};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const CANCELLATION_STRIDE: usize = 256;

impl SearchIndex {
    pub(super) fn content_has(&self, id: FileId, term: &str) -> bool {
        self.content_terms
            .get(term)
            .is_some_and(|positions| positions.contains_key(&id))
    }

    pub(super) fn add_content_scores(
        &self,
        scores: &mut HashMap<FileId, RankAccumulator>,
        term: &str,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<()> {
        cancellation.check()?;
        let Some(positions) = self.content_terms.get(term) else {
            return Ok(());
        };
        for (index, id) in positions.keys().enumerate() {
            if index % CANCELLATION_STRIDE == 0 {
                cancellation.check()?;
            }
            scores
                .entry(*id)
                .and_modify(|score| score.add(CONTENT, MatchReason::Content))
                .or_insert_with(|| RankAccumulator::new(CONTENT, MatchReason::Content));
        }
        Ok(())
    }

    pub(super) fn content_frequency(&self, id: FileId, term: &str) -> usize {
        self.content_terms
            .get(term)
            .and_then(|positions| positions.get(&id))
            .map(|positions| positions.len().max(1))
            .unwrap_or(0)
    }

    pub(super) fn content_matches_phrase(&self, id: FileId, phrase: &str) -> bool {
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

    pub(super) fn record_phrase_ids(&self, phrase: &str) -> Vec<FileId> {
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

    pub(super) fn content_phrase_ids(&self, phrase: &str) -> Vec<FileId> {
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

    pub(super) fn content_proximity_ids(&self, proximity: &QueryProximity) -> Vec<FileId> {
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

    pub(super) fn content_matches_proximity(&self, id: FileId, proximity: &QueryProximity) -> bool {
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

fn sorted_contains_position(positions: &[u32], position: u32) -> bool {
    debug_assert!(positions.windows(2).all(|window| window[0] < window[1]));
    positions.binary_search(&position).is_ok()
}

pub(super) fn sorted_has_position_within(positions: &[u32], anchor: u32, distance: u32) -> bool {
    debug_assert!(positions.windows(2).all(|window| window[0] < window[1]));
    let min = anchor.saturating_sub(distance);
    let max = anchor.saturating_add(distance);
    let index = positions.partition_point(|position| *position < min);
    positions
        .get(index)
        .is_some_and(|position| *position <= max)
}
