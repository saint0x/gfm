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

    pub(super) fn content_matches_phrase_cancellable(
        &self,
        id: FileId,
        phrase: &str,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<bool> {
        cancellation.check()?;
        let terms = tokenize(&normalize(phrase));
        if terms.is_empty() {
            return Ok(false);
        }
        if terms.len() == 1 {
            return Ok(self.content_has(id, &terms[0]));
        }

        let mut positions = Vec::with_capacity(terms.len());
        for term in &terms {
            let Some(term_positions) = self
                .content_terms
                .get(term)
                .and_then(|positions| positions.get(&id))
                .filter(|positions| !positions.is_empty())
            else {
                return Ok(false);
            };
            positions.push(term_positions);
        }

        let Some((anchor_offset, anchor_positions)) = positions
            .iter()
            .enumerate()
            .min_by_key(|(_, positions)| positions.len())
        else {
            return Ok(false);
        };
        for (index, anchor) in anchor_positions.iter().copied().enumerate() {
            if index % CANCELLATION_STRIDE == 0 {
                cancellation.check()?;
            }
            let Some(start) = anchor.checked_sub(anchor_offset as u32) else {
                continue;
            };
            if positions
                .iter()
                .enumerate()
                .all(|(offset, term_positions)| {
                    start
                        .checked_add(offset as u32)
                        .is_some_and(|position| sorted_contains_position(term_positions, position))
                })
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn record_phrase_ids_cancellable(
        &self,
        phrase: &str,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<Vec<FileId>> {
        cancellation.check()?;
        let terms = tokenize(&normalize(phrase));
        if terms.is_empty() {
            let mut ids = Vec::new();
            for (index, record) in self.records.values().enumerate() {
                if index % CANCELLATION_STRIDE == 0 {
                    cancellation.check()?;
                }
                if self.record_matches_phrase(record, phrase) {
                    ids.push(record.id);
                }
            }
            return Ok(ids);
        }

        let mut ids = BTreeSet::new();
        self.add_record_phrase_ids_for_field_cancellable(
            &terms,
            phrase,
            &self.name_terms,
            |columns, phrase| columns.matches_name_phrase(phrase),
            &mut ids,
            cancellation,
        )?;
        self.add_record_phrase_ids_for_field_cancellable(
            &terms,
            phrase,
            &self.path_terms,
            |columns, phrase| columns.matches_path_phrase(phrase),
            &mut ids,
            cancellation,
        )?;
        self.add_record_phrase_ids_for_field_cancellable(
            &terms,
            phrase,
            &self.metadata_terms,
            |columns, phrase| columns.matches_comment_phrase(phrase),
            &mut ids,
            cancellation,
        )?;
        Ok(ids.into_iter().collect())
    }

    fn add_record_phrase_ids_for_field_cancellable(
        &self,
        terms: &[String],
        phrase: &str,
        postings: &BTreeMap<String, BTreeSet<FileId>>,
        matches: impl Fn(&RecordColumns, &str) -> bool,
        ids: &mut BTreeSet<FileId>,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<()> {
        cancellation.check()?;
        let Some(candidates) = rarest_term_postings(terms, postings) else {
            return Ok(());
        };
        for (index, id) in candidates.iter().copied().enumerate() {
            if index % CANCELLATION_STRIDE == 0 {
                cancellation.check()?;
            }
            if self
                .columns
                .get(&id)
                .is_some_and(|columns| matches(columns, phrase))
            {
                ids.insert(id);
            }
        }
        Ok(())
    }

    pub(super) fn content_phrase_ids_cancellable(
        &self,
        phrase: &str,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<Vec<FileId>> {
        cancellation.check()?;
        let terms = tokenize(&normalize(phrase));
        let Some(candidates) = rarest_content_postings(&terms, &self.content_terms) else {
            return Ok(Vec::new());
        };
        let mut ids = Vec::new();
        for (index, id) in candidates.keys().copied().enumerate() {
            if index % CANCELLATION_STRIDE == 0 {
                cancellation.check()?;
            }
            if self.content_matches_phrase_cancellable(id, phrase, cancellation)? {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    pub(super) fn content_proximity_ids_cancellable(
        &self,
        proximity: &QueryProximity,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<Vec<FileId>> {
        cancellation.check()?;
        let Some((anchor_term, rarest)) =
            rarest_content_term_postings(&proximity.terms, &self.content_terms)
        else {
            return Ok(Vec::new());
        };
        let mut ids = Vec::new();
        for (index, id) in rarest.keys().copied().enumerate() {
            if index % CANCELLATION_STRIDE == 0 {
                cancellation.check()?;
            }
            if proximity.terms.iter().all(|term| {
                term == anchor_term
                    || self
                        .content_terms
                        .get(term)
                        .is_some_and(|positions| positions.contains_key(&id))
            }) && self.content_matches_proximity_cancellable(id, proximity, cancellation)?
            {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    pub(super) fn content_matches_proximity_cancellable(
        &self,
        id: FileId,
        proximity: &QueryProximity,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<bool> {
        cancellation.check()?;
        let mut positions = Vec::with_capacity(proximity.terms.len());
        for term in &proximity.terms {
            let Some(term_positions) = self
                .content_terms
                .get(term)
                .and_then(|positions| positions.get(&id))
                .filter(|positions| !positions.is_empty())
            else {
                return Ok(false);
            };
            positions.push(term_positions);
        }

        let Some((anchor_index, anchor_positions)) = positions
            .iter()
            .enumerate()
            .min_by_key(|(_, positions)| positions.len())
        else {
            return Ok(false);
        };
        for (position_index, anchor) in anchor_positions.iter().copied().enumerate() {
            if position_index % CANCELLATION_STRIDE == 0 {
                cancellation.check()?;
            }
            if positions.iter().enumerate().all(|(index, other)| {
                index == anchor_index
                    || sorted_has_position_within(other, anchor, proximity.distance)
            }) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

fn rarest_content_postings<'a>(
    terms: &'a [String],
    postings: &'a BTreeMap<String, BTreeMap<FileId, Vec<u32>>>,
) -> Option<&'a BTreeMap<FileId, Vec<u32>>> {
    rarest_content_term_postings(terms, postings).map(|(_, ids)| ids)
}

fn rarest_content_term_postings<'a>(
    terms: &'a [String],
    postings: &'a BTreeMap<String, BTreeMap<FileId, Vec<u32>>>,
) -> Option<(&'a String, &'a BTreeMap<FileId, Vec<u32>>)> {
    let mut rarest = None;
    for term in terms {
        let ids = postings.get(term)?;
        if rarest.is_none_or(|(_, current): (&String, &BTreeMap<FileId, Vec<u32>>)| {
            ids.len() < current.len()
        }) {
            rarest = Some((term, ids));
        }
    }
    rarest
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

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_types::VolumeId;

    #[test]
    fn rarest_content_postings_selects_sparse_anchor() {
        let first = FileId::new(VolumeId(1), 1);
        let second = FileId::new(VolumeId(1), 2);
        let mut postings = BTreeMap::new();
        postings.insert(
            "common".to_string(),
            BTreeMap::from([(first, vec![0, 4]), (second, vec![1, 8])]),
        );
        postings.insert("rare".to_string(), BTreeMap::from([(second, vec![8])]));

        let terms = vec!["common".to_string(), "rare".to_string()];
        let rarest = rarest_content_postings(&terms, &postings).unwrap();

        assert_eq!(rarest, postings.get("rare").unwrap());
    }

    #[test]
    fn rarest_content_term_postings_returns_anchor_term_and_postings() {
        let first = FileId::new(VolumeId(1), 1);
        let second = FileId::new(VolumeId(1), 2);
        let mut postings = BTreeMap::new();
        postings.insert(
            "common".to_string(),
            BTreeMap::from([(first, vec![0]), (second, vec![2])]),
        );
        postings.insert("rare".to_string(), BTreeMap::from([(second, vec![2])]));

        let terms = vec!["common".to_string(), "rare".to_string()];
        let (term, rarest) = rarest_content_term_postings(&terms, &postings).unwrap();

        assert_eq!(term, "rare");
        assert_eq!(rarest, postings.get("rare").unwrap());
    }

    #[test]
    fn rarest_content_postings_fails_closed_when_term_is_missing() {
        let mut postings = BTreeMap::new();
        postings.insert(
            "present".to_string(),
            BTreeMap::from([(FileId::new(VolumeId(1), 1), vec![0])]),
        );

        let terms = vec!["present".to_string(), "missing".to_string()];

        assert!(rarest_content_postings(&terms, &postings).is_none());
    }
}
