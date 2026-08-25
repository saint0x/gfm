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
use intent::{intent_score, term_matches_intent};
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
use gfm_types::{ContentPositions, ContentPosting, FileId, FileRecord, MatchReason, SearchHit};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const FUZZY_MIN_TERM_LEN: usize = 2;
const FUZZY_MAX_TERM_LEN: usize = 32;
const PREFIX_MIN_TERM_LEN: usize = 1;
const PREFIX_MAX_TERM_LEN: usize = 32;

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

#[derive(Debug, Clone, Default)]
pub struct SearchIndex {
    records: HashMap<FileId, FileRecord>,
    columns: HashMap<FileId, RecordColumns>,
    paths: HashMap<String, FileId>,
    name_exact: BTreeMap<String, BTreeSet<FileId>>,
    name_prefixes: BTreeMap<String, BTreeSet<FileId>>,
    name_terms: BTreeMap<String, BTreeSet<FileId>>,
    path_terms: BTreeMap<String, BTreeSet<FileId>>,
    metadata_terms: BTreeMap<String, BTreeSet<FileId>>,
    fuzzy_terms: BTreeMap<String, BTreeSet<String>>,
    extension: BTreeMap<String, BTreeSet<FileId>>,
    tags: BTreeMap<String, BTreeSet<FileId>>,
    content_terms: BTreeMap<String, BTreeMap<FileId, Vec<u32>>>,
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
        self.add_terms(&record, &normalized);
        self.paths.insert(path_key(&record.path), id);
        self.columns.insert(id, normalized);
        self.records.insert(id, record);
        true
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

    pub fn insert_content(&mut self, id: FileId, text: &str) {
        if !self.records.contains_key(&id) {
            return;
        }
        for (position, token) in tokenize(&normalize(text)).into_iter().enumerate() {
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
                    self.content_terms
                        .entry(term.clone())
                        .or_default()
                        .entry(*id)
                        .or_default();
                }
            }
            for positions in &posting.positions {
                if self.records.contains_key(&positions.id) {
                    self.content_terms
                        .entry(term.clone())
                        .or_default()
                        .insert(positions.id, positions.positions.clone());
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
        self.query_pass(query, limit, SearchPass::Full, cancellation)
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

        let hot = self.query_pass(query, limit, SearchPass::Hot, cancellation)?;
        let full = self.query_pass(query, limit, SearchPass::Full, cancellation)?;
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
        cancellation: &Cancellation,
    ) -> gfm_types::Result<Vec<SearchHit>> {
        cancellation.check()?;
        if query.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let mut scores: HashMap<FileId, RankAccumulator> = HashMap::new();
        let text = query.terms.join(" ");

        if !text.is_empty() {
            if let Some(ids) = self.name_exact.get(&text) {
                add_scores(&mut scores, ids, EXACT_NAME, MatchReason::ExactName);
            }
        }

        if !text.is_empty() {
            if let Some(ids) = self.name_prefix_ids(&text) {
                add_scores(&mut scores, ids, PREFIX_NAME, MatchReason::PrefixName);
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
                if let Some(ids) = self.content_ids(term) {
                    add_scores(&mut scores, &ids, CONTENT, MatchReason::Content);
                }
            }
        }

        if pass.includes_deep() {
            for term in &query.terms {
                cancellation.check()?;
                for id in self.fuzzy_ids(term) {
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
            for record in self.records.values().filter(|record| {
                self.record_matches_phrase(record, phrase)
                    || (pass.includes_deep() && self.content_matches_phrase(record.id, phrase))
            }) {
                cancellation.check()?;
                scores
                    .entry(record.id)
                    .and_modify(|score| score.add(PHRASE, MatchReason::PathComponent))
                    .or_insert_with(|| {
                        if pass.includes_deep() && self.content_matches_phrase(record.id, phrase) {
                            RankAccumulator::new(PHRASE, MatchReason::Content)
                        } else {
                            RankAccumulator::new(PHRASE, MatchReason::PathComponent)
                        }
                    });
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
            for record in self.records.values() {
                cancellation.check()?;
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

        for record in self.records.values() {
            cancellation.check()?;
            let score = intent_score(query, record);
            if score > 0 {
                scores
                    .entry(record.id)
                    .and_modify(|current| current.boost(score))
                    .or_insert_with(|| RankAccumulator::new(score, MatchReason::PathComponent));
            }
        }

        if query
            .expression
            .as_ref()
            .is_some_and(expression_needs_universe)
            || (scores.is_empty() && (!query.filters.is_empty() || !query.phrases.is_empty()))
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

        let mut hits: Vec<_> = scores
            .into_iter()
            .filter_map(|(id, score)| {
                if cancellation.check().is_err() {
                    return None;
                }
                self.records
                    .get(&id)
                    .filter(|record| self.record_matches_query(record, query, pass))
                    .map(|record| {
                        let (score, reason) = score.finish();
                        SearchHit {
                            record: record.clone(),
                            score,
                            reason,
                            snippet: None,
                        }
                    })
            })
            .collect();

        sort_hits(&mut hits);
        hits.truncate(limit);
        cancellation.check()?;
        Ok(hits)
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

    fn content_ids(&self, term: &str) -> Option<BTreeSet<FileId>> {
        self.content_terms
            .get(term)
            .map(|positions| positions.keys().copied().collect())
    }

    fn content_frequency(&self, id: FileId, term: &str) -> usize {
        self.content_terms
            .get(term)
            .and_then(|positions| positions.get(&id))
            .map(|positions| positions.len().max(1))
            .unwrap_or(0)
    }

    fn fuzzy_ids(&self, term: &str) -> BTreeSet<FileId> {
        if !is_fuzzy_term(term) {
            return BTreeSet::new();
        }
        let mut ids = BTreeSet::new();
        for key in deletion_keys(term, 2) {
            if let Some(candidates) = self.fuzzy_terms.get(&key) {
                for candidate in candidates {
                    if bounded_levenshtein(candidate, term, 2).is_some() {
                        if let Some(matches) = self.name_terms.get(candidate) {
                            ids.extend(matches);
                        }
                    }
                }
            }
        }
        ids
    }

    fn name_prefix_ids(&self, term: &str) -> Option<&BTreeSet<FileId>> {
        if !is_prefix_term(term) {
            return None;
        }
        self.name_prefixes.get(term)
    }

    fn content_matches_phrase(&self, id: FileId, phrase: &str) -> bool {
        let terms = tokenize(&normalize(phrase));
        if terms.is_empty() {
            return false;
        }
        if terms.len() == 1 {
            return self.content_has(id, &terms[0]);
        }

        let Some(first_positions) = self
            .content_terms
            .get(&terms[0])
            .and_then(|positions| positions.get(&id))
        else {
            return false;
        };
        if first_positions.is_empty() {
            return false;
        }

        let later: Option<Vec<BTreeSet<u32>>> = terms
            .iter()
            .skip(1)
            .map(|term| {
                self.content_terms
                    .get(term)
                    .and_then(|positions| positions.get(&id))
                    .filter(|positions| !positions.is_empty())
                    .map(|positions| positions.iter().copied().collect())
            })
            .collect();
        let Some(later) = later else {
            return false;
        };

        first_positions.iter().any(|start| {
            later
                .iter()
                .enumerate()
                .all(|(offset, positions)| positions.contains(&(*start + offset as u32 + 1)))
        })
    }

    fn content_proximity_ids(&self, proximity: &QueryProximity) -> BTreeSet<FileId> {
        let mut terms = proximity.terms.iter();
        let Some(first) = terms.next() else {
            return BTreeSet::new();
        };
        let Some(first_ids) = self.content_ids(first) else {
            return BTreeSet::new();
        };
        let candidates = terms.try_fold(first_ids, |mut candidates, term| {
            let ids = self.content_ids(term)?;
            candidates.retain(|id| ids.contains(id));
            Some(candidates)
        });
        candidates
            .unwrap_or_default()
            .into_iter()
            .filter(|id| self.content_matches_proximity(*id, proximity))
            .collect()
    }

    fn content_matches_proximity(&self, id: FileId, proximity: &QueryProximity) -> bool {
        let positions: Option<Vec<&Vec<u32>>> = proximity
            .terms
            .iter()
            .map(|term| {
                self.content_terms
                    .get(term)
                    .and_then(|positions| positions.get(&id))
                    .filter(|positions| !positions.is_empty())
            })
            .collect();
        let Some(positions) = positions else {
            return false;
        };
        let mut anchors = positions[0].iter().copied();
        anchors.any(|anchor| {
            positions.iter().skip(1).all(|other| {
                other
                    .iter()
                    .any(|position| anchor.abs_diff(*position) <= proximity.distance)
            })
        })
    }

    fn add_terms(&mut self, record: &FileRecord, columns: &RecordColumns) {
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
            for prefix in token_prefixes(token) {
                self.name_prefixes
                    .entry(prefix)
                    .or_default()
                    .insert(record.id);
            }
            if is_new {
                self.add_fuzzy_term(token);
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

    fn remove_terms(&mut self, record: &FileRecord) {
        let columns = self
            .columns
            .get(&record.id)
            .cloned()
            .unwrap_or_else(|| RecordColumns::from_record(record));
        remove_id(&mut self.name_exact, &columns.name, record.id);
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
        for tag in &columns.tags {
            remove_id(&mut self.tags, tag, record.id);
        }
        for token in &columns.metadata_tokens {
            remove_id(&mut self.metadata_terms, token, record.id);
        }
        for positions in self.content_terms.values_mut() {
            positions.remove(&record.id);
        }
        self.content_terms
            .retain(|_, positions| !positions.is_empty());
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
    (FUZZY_MIN_TERM_LEN..=FUZZY_MAX_TERM_LEN).contains(&term.chars().count())
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

fn expression_needs_universe(expression: &QueryExpr) -> bool {
    match expression {
        QueryExpr::Filter(_) | QueryExpr::Not(_) => true,
        QueryExpr::And(expressions) | QueryExpr::Or(expressions) => {
            expressions.iter().any(expression_needs_universe)
        }
        QueryExpr::Term(_) | QueryExpr::Phrase(_) | QueryExpr::Proximity(_) => false,
    }
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

fn remove_id(map: &mut BTreeMap<String, BTreeSet<FileId>>, key: &str, id: FileId) {
    if let Some(ids) = map.get_mut(key) {
        ids.remove(&id);
        if ids.is_empty() {
            map.remove(key);
        }
    }
}

pub(crate) fn sort_hits(hits: &mut [SearchHit]) {
    hits.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| {
                a.record
                    .name
                    .to_lowercase()
                    .cmp(&b.record.name.to_lowercase())
            })
            .then_with(|| {
                a.record
                    .path
                    .to_string_lossy()
                    .cmp(&b.record.path.to_string_lossy())
            })
            .then_with(|| a.record.id.cmp(&b.record.id))
    });
}

#[cfg(test)]
mod tests;
