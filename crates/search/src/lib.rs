mod candidates;
mod columns;
mod content;
mod fuzzy;
mod hits;
mod intent;
mod lookup;
mod matchers;
mod query;
mod ranking;
mod scoring;
mod session;
mod shard;
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
use fuzzy::deletion_keys;
use hits::BoundedHitMerge;
use intent::QueryIntent;
pub use lookup::{
    EmptySearchLookup, SearchLookup, SearchLookupBudget, SearchLookupIds, SearchLookupTelemetry,
    SearchLookupTerms,
};
use query::{normalize, tokenize};
pub use query::{
    DateComparison, DateField, QueryExpr, QueryFilter, QueryKind, QueryProximity, QueryScope,
    SearchQuery, SizeComparison,
};
use ranking::{
    RankAccumulator, CONTENT, EXACT_NAME, EXTENSION, FUZZY_NAME, NAME_TOKEN, PATH_COMPONENT,
    PHRASE, PREFIX_NAME, PROXIMITY, SUBSTRING_NAME, TAG,
};
use scoring::{add_scores, seed_scores};
pub use session::SearchSupersession;
pub use shard::ShardedSearchIndex;
use terms::{
    is_fuzzy_term, is_prefix_term, is_substring_gram, path_key, substring_grams, token_prefixes,
};

use gfm_jobs::Cancellation;
use gfm_types::{
    ContentPositions, ContentPosting, FileId, FileKind, FileRecord, MatchReason, SearchHit,
};
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

    pub fn indexed_name_prefixes(&self) -> usize {
        self.name_prefixes.len()
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
        self.insert_with_columns_inner(record, columns, true, true, true, true)
    }

    pub fn insert_with_columns_deferred_fuzzy(
        &mut self,
        record: FileRecord,
        columns: SearchRecordColumns,
    ) -> bool {
        self.insert_with_columns_inner(record, columns, true, true, false, true)
    }

    pub fn insert_with_columns_deferred_sidecars(
        &mut self,
        record: FileRecord,
        columns: SearchRecordColumns,
    ) -> bool {
        self.insert_with_columns_inner(record, columns, false, false, false, false)
    }

    fn insert_with_columns_inner(
        &mut self,
        record: FileRecord,
        columns: SearchRecordColumns,
        build_prefixes: bool,
        build_substrings: bool,
        build_fuzzy: bool,
        build_metadata: bool,
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
        self.add_terms_with_sidecar_policy(
            &record,
            &normalized,
            build_prefixes,
            build_substrings,
            build_fuzzy,
            build_metadata,
        );
        self.paths.insert(path_key(&record.path), id);
        self.columns.insert(id, normalized);
        self.records.insert(id, record);
        true
    }

    pub fn import_prefix_postings(&mut self, postings: &[SearchPrefixPosting]) -> usize {
        for posting in postings {
            let prefix = normalize(&posting.prefix);
            if !is_prefix_term(&prefix) {
                continue;
            }
            let ids = posting
                .ids
                .iter()
                .copied()
                .filter(|id| self.records.contains_key(id))
                .collect::<BTreeSet<_>>();
            if !ids.is_empty() {
                self.name_prefixes.entry(prefix).or_default().extend(ids);
            }
        }
        self.name_prefixes.len()
    }

    pub fn import_substring_postings(&mut self, postings: &[SearchSubstringPosting]) -> usize {
        for posting in postings {
            let gram = normalize(&posting.gram);
            if !is_substring_gram(&gram) {
                continue;
            }
            let ids = posting
                .ids
                .iter()
                .copied()
                .filter(|id| self.records.contains_key(id))
                .collect::<BTreeSet<_>>();
            if !ids.is_empty() {
                self.name_substrings.entry(gram).or_default().extend(ids);
            }
        }
        self.name_substrings.len()
    }

    pub fn import_fuzzy_postings(&mut self, postings: &[SearchFuzzyPosting]) -> usize {
        for posting in postings {
            let key = normalize(&posting.key);
            if key.is_empty() {
                continue;
            }
            let terms = posting
                .terms
                .iter()
                .map(|term| normalize(term))
                .filter(|term| is_fuzzy_term(term))
                .collect::<BTreeSet<_>>();
            if !terms.is_empty() {
                self.fuzzy_terms.entry(key).or_default().extend(terms);
            }
        }
        self.fuzzy_terms.len()
    }

    pub fn import_metadata_postings(&mut self, postings: &[SearchMetadataPosting]) -> usize {
        for posting in postings {
            let term = normalize(&posting.term);
            if term.is_empty() {
                continue;
            }
            let ids = posting
                .ids
                .iter()
                .copied()
                .filter(|id| self.records.contains_key(id))
                .collect::<BTreeSet<_>>();
            if ids.is_empty() {
                continue;
            }
            match posting.field {
                SearchMetadataField::Tag => self.tags.entry(term).or_default().extend(ids),
                SearchMetadataField::Comment => {
                    self.metadata_terms.entry(term).or_default().extend(ids)
                }
            }
        }
        self.tags.len() + self.metadata_terms.len()
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

    #[cfg(test)]
    fn content_record_term_count(&self, id: FileId) -> usize {
        self.content_record_terms
            .get(&id)
            .map(BTreeSet::len)
            .unwrap_or(0)
    }

    pub fn insert_content(&mut self, id: FileId, text: &str) {
        if !self.records.contains_key(&id) {
            return;
        }
        self.remove_content(id);
        for (position, token) in tokenize(&normalize(text)).into_iter().enumerate() {
            self.content_record_terms
                .entry(id)
                .or_default()
                .insert(token.clone());
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
                self.content_record_terms
                    .entry(id)
                    .or_default()
                    .insert(term.clone());
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
                    self.content_record_terms
                        .entry(*id)
                        .or_default()
                        .insert(term.clone());
                    self.content_terms
                        .entry(term.clone())
                        .or_default()
                        .entry(*id)
                        .or_default();
                }
            }
            for positions in &posting.positions {
                if self.records.contains_key(&positions.id) {
                    let mut normalized_positions = positions.positions.clone();
                    normalized_positions.sort_unstable();
                    normalized_positions.dedup();
                    self.content_record_terms
                        .entry(positions.id)
                        .or_default()
                        .insert(term.clone());
                    self.content_terms
                        .entry(term.clone())
                        .or_default()
                        .insert(positions.id, normalized_positions);
                }
            }
        }
    }

    pub fn remove_content(&mut self, id: FileId) {
        let Some(terms) = self.content_record_terms.remove(&id) else {
            for positions in self.content_terms.values_mut() {
                positions.remove(&id);
            }
            self.content_terms
                .retain(|_, positions| !positions.is_empty());
            return;
        };
        for term in terms {
            if let Some(positions) = self.content_terms.get_mut(&term) {
                positions.remove(&id);
                if positions.is_empty() {
                    self.content_terms.remove(&term);
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
            .and_then(|expression| self.expression_candidate_ids(expression, pass));

        if !text.is_empty() {
            if let Some(ids) = self.name_exact.get(&text) {
                add_scores(&mut scores, ids, EXACT_NAME, MatchReason::ExactName);
            }
        }

        if !text.is_empty() {
            let ids = self.name_prefix_ids(&text, lookup, budget, &mut telemetry)?;
            if !ids.is_empty() {
                add_scores(&mut scores, &ids, PREFIX_NAME, MatchReason::PrefixName);
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
                self.add_content_scores(&mut scores, term);
            }
        }

        if pass.includes_deep() {
            for term in &query.terms {
                cancellation.check()?;
                for id in self.fuzzy_ids(term, lookup, budget, &mut telemetry)? {
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
            let candidates = self.name_substring_ids(&text, lookup, budget, &mut telemetry)?;
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
            seed_scores(&mut scores, ids);
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

    fn query_simple_single_term_pass(
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
            add_scores(&mut scores, ids, EXACT_NAME, MatchReason::ExactName);
        }
        let ids = self.name_prefix_ids(term, lookup, budget, &mut telemetry)?;
        if !ids.is_empty() {
            add_scores(&mut scores, &ids, PREFIX_NAME, MatchReason::PrefixName);
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
            add_scores(&mut scores, postings.0, postings.1, postings.2);
        }
        if pass.includes_deep() {
            for id in self.fuzzy_ids(term, lookup, budget, &mut telemetry)? {
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

    fn query_simple_multi_term_pass(
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
        let full_text_prefix_ids = self.name_prefix_ids(&text, lookup, budget, &mut telemetry)?;
        let mut fuzzy_by_term = Vec::with_capacity(query.terms.len());
        let mut candidate_sets = Vec::with_capacity(query.terms.len());
        for term in &query.terms {
            cancellation.check()?;
            let fuzzy_ids = if pass.includes_deep() {
                self.fuzzy_ids(term, lookup, budget, &mut telemetry)?
            } else {
                BTreeSet::new()
            };
            let mut candidates = self.term_candidate_ids(term, pass);
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

    fn add_terms(&mut self, record: &FileRecord, columns: &RecordColumns) {
        self.add_terms_with_sidecar_policy(record, columns, true, true, true, true);
    }

    fn add_terms_with_sidecar_policy(
        &mut self,
        record: &FileRecord,
        columns: &RecordColumns,
        build_prefixes: bool,
        build_substrings: bool,
        build_fuzzy: bool,
        build_metadata: bool,
    ) {
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
            if build_prefixes {
                for prefix in token_prefixes(token) {
                    self.name_prefixes
                        .entry(prefix)
                        .or_default()
                        .insert(record.id);
                }
            }
            if build_fuzzy && is_new {
                self.add_fuzzy_term(token);
            }
        }
        if build_substrings {
            for gram in substring_grams(&columns.name) {
                self.name_substrings
                    .entry(gram)
                    .or_default()
                    .insert(record.id);
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
        self.kind.entry(record.kind).or_default().insert(record.id);
        if build_metadata {
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
    }

    fn remove_terms(&mut self, record: &FileRecord) {
        let columns = self
            .columns
            .get(&record.id)
            .cloned()
            .unwrap_or_else(|| RecordColumns::from_record(record));
        remove_id(&mut self.name_exact, &columns.name, record.id);
        for gram in substring_grams(&columns.name) {
            remove_id(&mut self.name_substrings, &gram, record.id);
        }
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
        if let Some(ids) = self.kind.get_mut(&record.kind) {
            ids.remove(&record.id);
            if ids.is_empty() {
                self.kind.remove(&record.kind);
            }
        }
        for tag in &columns.tags {
            remove_id(&mut self.tags, tag, record.id);
        }
        for token in &columns.metadata_tokens {
            remove_id(&mut self.metadata_terms, token, record.id);
        }
        self.remove_content(record.id);
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

fn rarest_term_postings<'a>(
    terms: &[String],
    postings: &'a BTreeMap<String, BTreeSet<FileId>>,
) -> Option<&'a BTreeSet<FileId>> {
    terms
        .iter()
        .map(|term| postings.get(term))
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .min_by_key(|ids| ids.len())
}

fn remove_id(map: &mut BTreeMap<String, BTreeSet<FileId>>, key: &str, id: FileId) {
    if let Some(ids) = map.get_mut(key) {
        ids.remove(&id);
        if ids.is_empty() {
            map.remove(key);
        }
    }
}

#[cfg(test)]
pub(crate) use hits::sort_hits;
pub(crate) use hits::top_hits;

#[cfg(test)]
mod tests;
