mod fuzzy;
mod intent;
mod query;
mod ranking;
mod session;
mod shard;

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
    paths: HashMap<String, FileId>,
    name_exact: BTreeMap<String, BTreeSet<FileId>>,
    name_terms: BTreeMap<String, BTreeSet<FileId>>,
    path_terms: BTreeMap<String, BTreeSet<FileId>>,
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
            self.paths.remove(&path_key(&old.path));
        }
        self.add_terms(&record);
        self.paths.insert(path_key(&record.path), id);
        self.records.insert(id, record);
    }

    pub fn remove(&mut self, id: FileId) -> Option<FileRecord> {
        let record = self.records.remove(&id)?;
        self.remove_terms(&record);
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
            for (term, ids) in self.name_terms.range(text.clone()..) {
                cancellation.check()?;
                if !term.starts_with(&text) {
                    break;
                }
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
                    if !record_contains_term(record, term)
                        && record_fuzzy_matches_term(record, term)
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
                record_matches_phrase(record, phrase)
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
                let name = normalize(&record.name);
                if !text.is_empty() && name.contains(&text) {
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
            record_contains_term(record, term)
                || (pass.includes_deep() && self.content_has(record.id, term))
        }) {
            return false;
        }
        if !query.phrases.iter().all(|phrase| {
            record_matches_phrase(record, phrase)
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
        query.filters.iter().all(|filter| filter.matches(record))
    }

    fn record_matches_expression(
        &self,
        record: &FileRecord,
        expression: &QueryExpr,
        pass: SearchPass,
    ) -> bool {
        match expression {
            QueryExpr::Term(term) => {
                record_contains_term(record, term)
                    || (pass.includes_deep() && self.content_has(record.id, term))
                    || (pass.includes_deep() && record_fuzzy_matches_term(record, term))
                    || term_matches_intent(term, record)
            }
            QueryExpr::Phrase(phrase) => {
                record_matches_phrase(record, phrase)
                    || (pass.includes_deep() && self.content_matches_phrase(record.id, phrase))
            }
            QueryExpr::Proximity(proximity) => {
                pass.includes_deep() && self.content_matches_proximity(record.id, proximity)
            }
            QueryExpr::Filter(filter) => filter.matches(record),
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
        if self.pinned.contains(&record.id) {
            score += USER_PINNED;
        }
        for filter in &query.filters {
            if filter_kind_matches(filter, record.kind) {
                score += kind_score(record.kind);
            } else if filter.matches(record) {
                score += KIND_MATCH / 3;
            }
        }
        for term in &query.terms {
            score += capped_frequency(count_term(&normalize(&record.name), term), NAME_FREQUENCY);
            score += capped_frequency(
                count_term(&normalize_path(&record.path), term),
                PATH_FREQUENCY,
            );
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

    fn add_terms(&mut self, record: &FileRecord) {
        let name = normalize(&record.name);
        self.name_exact
            .entry(name.clone())
            .or_default()
            .insert(record.id);
        for token in tokenize(&name) {
            let is_new = !self.name_terms.contains_key(&token);
            self.name_terms
                .entry(token.clone())
                .or_default()
                .insert(record.id);
            if is_new {
                self.add_fuzzy_term(&token);
            }
        }
        for token in record
            .path
            .components()
            .filter_map(|component| component.as_os_str().to_str())
            .flat_map(|component| tokenize(&normalize(component)))
        {
            self.path_terms.entry(token).or_default().insert(record.id);
        }
        if let Some(ext) = record.extension() {
            self.extension
                .entry(normalize(ext))
                .or_default()
                .insert(record.id);
        }
        for tag in &record.tags {
            let tag = normalize(tag);
            if !tag.is_empty() {
                self.tags.entry(tag).or_default().insert(record.id);
            }
        }
    }

    fn remove_terms(&mut self, record: &FileRecord) {
        let name = normalize(&record.name);
        remove_id(&mut self.name_exact, &name, record.id);
        for token in tokenize(&name) {
            remove_id(&mut self.name_terms, &token, record.id);
            if !self.name_terms.contains_key(&token) {
                self.remove_fuzzy_term(&token);
            }
        }
        for token in record
            .path
            .components()
            .filter_map(|component| component.as_os_str().to_str())
            .flat_map(|component| tokenize(&normalize(component)))
        {
            remove_id(&mut self.path_terms, &token, record.id);
        }
        if let Some(ext) = record.extension() {
            remove_id(&mut self.extension, &normalize(ext), record.id);
        }
        for tag in &record.tags {
            remove_id(&mut self.tags, &normalize(tag), record.id);
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

fn record_contains_term(record: &FileRecord, term: &str) -> bool {
    normalize(&record.name).contains(term)
        || normalize_path(&record.path).contains(term)
        || record.tags.iter().any(|tag| normalize(tag).contains(term))
}

fn record_matches_phrase(record: &FileRecord, phrase: &str) -> bool {
    normalize(&record.name).contains(phrase) || normalize_path(&record.path).contains(phrase)
}

fn record_fuzzy_matches_term(record: &FileRecord, term: &str) -> bool {
    fuzzy_record_terms(record)
        .into_iter()
        .any(|candidate| bounded_levenshtein(&candidate, term, 2).is_some())
}

fn fuzzy_record_terms(record: &FileRecord) -> BTreeSet<String> {
    tokenize(&normalize(&record.name))
        .into_iter()
        .filter(|term| is_fuzzy_term(term))
        .collect()
}

fn is_fuzzy_term(term: &str) -> bool {
    (FUZZY_MIN_TERM_LEN..=FUZZY_MAX_TERM_LEN).contains(&term.chars().count())
}

fn normalize_path(path: &std::path::Path) -> String {
    normalize(&path.to_string_lossy())
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
