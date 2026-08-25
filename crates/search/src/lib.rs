mod fuzzy;
mod intent;
mod query;
mod session;
mod shard;

use fuzzy::bounded_levenshtein;
use intent::{intent_score, term_matches_intent};
use query::{normalize, tokenize};
pub use query::{
    DateComparison, DateField, QueryExpr, QueryFilter, QueryKind, QueryScope, SearchQuery,
    SizeComparison,
};
pub use session::SearchSupersession;
pub use shard::ShardedSearchIndex;

use gfm_jobs::Cancellation;
use gfm_types::{ContentPositions, ContentPosting, FileId, FileRecord, MatchReason, SearchHit};
use std::collections::{BTreeMap, BTreeSet, HashMap};

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
    extension: BTreeMap<String, BTreeSet<FileId>>,
    tags: BTreeMap<String, BTreeSet<FileId>>,
    content_terms: BTreeMap<String, BTreeMap<FileId, Vec<u32>>>,
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

        let mut scores: HashMap<FileId, (i64, MatchReason)> = HashMap::new();
        let text = query.terms.join(" ");

        if !text.is_empty() {
            if let Some(ids) = self.name_exact.get(&text) {
                add_scores(&mut scores, ids, 1_000, MatchReason::ExactName);
            }
        }

        if !text.is_empty() {
            for (term, ids) in self.name_terms.range(text.clone()..) {
                cancellation.check()?;
                if !term.starts_with(&text) {
                    break;
                }
                add_scores(&mut scores, ids, 700, MatchReason::PrefixName);
            }
        }

        for term in &query.terms {
            cancellation.check()?;
            if let Some(ids) = self.name_terms.get(term) {
                add_scores(&mut scores, ids, 500, MatchReason::SubstringName);
            }
            if let Some(ids) = self.path_terms.get(term) {
                add_scores(&mut scores, ids, 250, MatchReason::PathComponent);
            }
            if let Some(ids) = self.extension.get(term) {
                add_scores(&mut scores, ids, 350, MatchReason::Extension);
            }
            if let Some(ids) = self.tags.get(term) {
                add_scores(&mut scores, ids, 325, MatchReason::Tag);
            }
            if pass.includes_deep() {
                if let Some(ids) = self.content_ids(term) {
                    add_scores(&mut scores, &ids, 150, MatchReason::Content);
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
                    .and_modify(|(score, _)| *score += 450)
                    .or_insert_with(|| {
                        if pass.includes_deep() && self.content_matches_phrase(record.id, phrase) {
                            (450, MatchReason::Content)
                        } else {
                            (450, MatchReason::PathComponent)
                        }
                    });
            }
        }

        if scores.len() < limit {
            for record in self.records.values() {
                cancellation.check()?;
                let name = normalize(&record.name);
                if !text.is_empty() && name.contains(&text) {
                    scores
                        .entry(record.id)
                        .and_modify(|(score, _)| *score += 300)
                        .or_insert((300, MatchReason::SubstringName));
                } else if pass.includes_deep()
                    && !text.is_empty()
                    && bounded_levenshtein(&name, &text, 2).is_some()
                {
                    scores
                        .entry(record.id)
                        .and_modify(|(score, _)| *score += 100)
                        .or_insert((100, MatchReason::FuzzyName));
                }
            }
        }

        for record in self.records.values() {
            cancellation.check()?;
            let score = intent_score(query, record);
            if score > 0 {
                scores
                    .entry(record.id)
                    .and_modify(|(current, _)| *current += score)
                    .or_insert((score, MatchReason::PathComponent));
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
                    .or_insert((0, MatchReason::PathComponent));
            }
        }

        let mut hits: Vec<_> = scores
            .into_iter()
            .filter_map(|(id, (score, reason))| {
                if cancellation.check().is_err() {
                    return None;
                }
                self.records
                    .get(&id)
                    .filter(|record| self.record_matches_query(record, query, pass))
                    .map(|record| SearchHit {
                        record: record.clone(),
                        score: score + recency_score(record),
                        reason,
                        snippet: None,
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

    fn add_terms(&mut self, record: &FileRecord) {
        let name = normalize(&record.name);
        self.name_exact
            .entry(name.clone())
            .or_default()
            .insert(record.id);
        for token in tokenize(&name) {
            self.name_terms.entry(token).or_default().insert(record.id);
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
    let name = normalize(&record.name);
    bounded_levenshtein(&name, term, 2).is_some()
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
        QueryExpr::Term(_) | QueryExpr::Phrase(_) => false,
    }
}

fn add_scores(
    scores: &mut HashMap<FileId, (i64, MatchReason)>,
    ids: &BTreeSet<FileId>,
    points: i64,
    reason: MatchReason,
) {
    for id in ids {
        scores
            .entry(*id)
            .and_modify(|(score, _)| *score += points)
            .or_insert((points, reason.clone()));
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

fn recency_score(record: &FileRecord) -> i64 {
    record
        .modified
        .and_then(|time| time.elapsed().ok())
        .map(|age| {
            let days = age.as_secs() / 86_400;
            100i64.saturating_sub(days.min(100) as i64)
        })
        .unwrap_or(0)
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
