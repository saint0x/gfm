mod query;

use query::{normalize, tokenize};
pub use query::{
    DateComparison, DateField, QueryExpr, QueryFilter, QueryKind, QueryScope, SearchQuery,
    SizeComparison,
};

use gfm_types::{ContentPositions, ContentPosting, FileId, FileRecord, MatchReason, SearchHit};
use std::collections::{BTreeMap, BTreeSet, HashMap};

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
        if query.is_empty() || limit == 0 {
            return Vec::new();
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
                if !term.starts_with(&text) {
                    break;
                }
                add_scores(&mut scores, ids, 700, MatchReason::PrefixName);
            }
        }

        for term in &query.terms {
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
            if let Some(ids) = self.content_ids(term) {
                add_scores(&mut scores, &ids, 150, MatchReason::Content);
            }
        }

        for phrase in &query.phrases {
            for record in self.records.values().filter(|record| {
                record_matches_phrase(record, phrase)
                    || self.content_matches_phrase(record.id, phrase)
            }) {
                scores
                    .entry(record.id)
                    .and_modify(|(score, _)| *score += 450)
                    .or_insert_with(|| {
                        if self.content_matches_phrase(record.id, phrase) {
                            (450, MatchReason::Content)
                        } else {
                            (450, MatchReason::PathComponent)
                        }
                    });
            }
        }

        if scores.len() < limit {
            for record in self.records.values() {
                let name = normalize(&record.name);
                if !text.is_empty() && name.contains(&text) {
                    scores
                        .entry(record.id)
                        .and_modify(|(score, _)| *score += 300)
                        .or_insert((300, MatchReason::SubstringName));
                } else if !text.is_empty() && bounded_levenshtein(&name, &text, 2).is_some() {
                    scores
                        .entry(record.id)
                        .and_modify(|(score, _)| *score += 100)
                        .or_insert((100, MatchReason::FuzzyName));
                }
            }
        }

        if query
            .expression
            .as_ref()
            .is_some_and(expression_needs_universe)
            || (scores.is_empty() && (!query.filters.is_empty() || !query.phrases.is_empty()))
        {
            for record in self.records.values() {
                scores
                    .entry(record.id)
                    .or_insert((0, MatchReason::PathComponent));
            }
        }

        let mut hits: Vec<_> = scores
            .into_iter()
            .filter_map(|(id, (score, reason))| {
                self.records
                    .get(&id)
                    .filter(|record| self.record_matches_query(record, query))
                    .map(|record| SearchHit {
                        record: record.clone(),
                        score: score + recency_score(record),
                        reason,
                    })
            })
            .collect();

        hits.sort_by(|a, b| {
            b.score.cmp(&a.score).then_with(|| {
                a.record
                    .name
                    .to_lowercase()
                    .cmp(&b.record.name.to_lowercase())
            })
        });
        hits.truncate(limit);
        hits
    }

    fn record_matches_query(&self, record: &FileRecord, query: &SearchQuery) -> bool {
        if let Some(expression) = &query.expression {
            return self.record_matches_expression(record, expression);
        }
        if query
            .excluded_terms
            .iter()
            .any(|term| record_contains_term(record, term) || self.content_has(record.id, term))
        {
            return false;
        }
        if !query.phrases.iter().all(|phrase| {
            record_matches_phrase(record, phrase) || self.content_matches_phrase(record.id, phrase)
        }) {
            return false;
        }
        query.filters.iter().all(|filter| filter.matches(record))
    }

    fn record_matches_expression(&self, record: &FileRecord, expression: &QueryExpr) -> bool {
        match expression {
            QueryExpr::Term(term) => {
                record_contains_term(record, term) || self.content_has(record.id, term)
            }
            QueryExpr::Phrase(phrase) => {
                record_matches_phrase(record, phrase)
                    || self.content_matches_phrase(record.id, phrase)
            }
            QueryExpr::Filter(filter) => filter.matches(record),
            QueryExpr::Not(expression) => !self.record_matches_expression(record, expression),
            QueryExpr::And(expressions) => expressions
                .iter()
                .all(|expression| self.record_matches_expression(record, expression)),
            QueryExpr::Or(expressions) => expressions
                .iter()
                .any(|expression| self.record_matches_expression(record, expression)),
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

fn bounded_levenshtein(left: &str, right: &str, max: usize) -> Option<usize> {
    if left.len().abs_diff(right.len()) > max {
        return None;
    }
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0; right.len() + 1];

    for (i, left_ch) in left.chars().enumerate() {
        current[0] = i + 1;
        let mut row_min = current[0];
        for (j, right_ch) in right.chars().enumerate() {
            let cost = usize::from(left_ch != right_ch);
            current[j + 1] = (previous[j + 1] + 1)
                .min(current[j] + 1)
                .min(previous[j] + cost);
            row_min = row_min.min(current[j + 1]);
        }
        if row_min > max {
            return None;
        }
        std::mem::swap(&mut previous, &mut current);
    }

    (previous[right.len()] <= max).then_some(previous[right.len()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_types::{FileId, FileKind, FileRecord, VolumeId};
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn ranks_exact_name_above_path_component() {
        let mut index = SearchIndex::new();
        index.insert(record(1, "/Users/me/Desktop/report.pdf", "report.pdf"));
        index.insert(record(2, "/Users/me/report/archive.txt", "archive.txt"));

        let hits = index.query("report", 10);

        assert_eq!(hits[0].record.name, "report.pdf");
        assert_eq!(hits[0].reason, MatchReason::PrefixName);
        assert!(hits[0].score > hits[1].score);
    }

    #[test]
    fn removes_reindexed_records() {
        let mut index = SearchIndex::new();
        let mut item = record(1, "/tmp/alpha.txt", "alpha.txt");
        index.insert(item.clone());
        item.path = PathBuf::from("/tmp/beta.txt");
        item.name = "beta.txt".to_string();
        index.insert(item);

        assert!(index.query("alpha", 10).is_empty());
        assert_eq!(index.query("beta", 10).len(), 1);
    }

    #[test]
    fn removes_subtree_by_path() {
        let mut index = SearchIndex::new();
        index.insert(record(1, "/tmp/folder", "folder"));
        index.insert(record(2, "/tmp/folder/child.txt", "child.txt"));
        index.insert(record(3, "/tmp/other.txt", "other.txt"));

        let removed = index.remove_subtree("/tmp/folder");

        assert_eq!(removed.len(), 2);
        assert!(index.query("child", 10).is_empty());
        assert_eq!(index.query("other", 10).len(), 1);
    }

    #[test]
    fn finds_content_terms() {
        let mut index = SearchIndex::new();
        let item = record(1, "/tmp/notes.txt", "notes.txt");
        index.insert(item.clone());
        index.insert_content(
            item.id,
            "an elite file manager needs instant content search",
        );

        let hits = index.query("instant", 10);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].reason, MatchReason::Content);
    }

    #[test]
    fn matches_content_phrases_by_token_position() {
        let mut index = SearchIndex::new();
        let keep = record(1, "/tmp/keep.txt", "keep.txt");
        let skip = record(2, "/tmp/skip.txt", "skip.txt");
        index.insert(keep.clone());
        index.insert(skip.clone());
        index.insert_content(keep.id, "an instant content search result");
        index.insert_content(skip.id, "instant search content result");

        let hits = index.query(r#""instant content search""#, 10);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record.name, "keep.txt");
        assert_eq!(hits[0].reason, MatchReason::Content);
    }

    #[test]
    fn supports_boolean_content_phrase_queries() {
        let mut index = SearchIndex::new();
        let first = record(1, "/tmp/first.txt", "first.txt");
        let second = record(2, "/tmp/second.txt", "second.txt");
        index.insert(first.clone());
        index.insert(second.clone());
        index.insert_content(first.id, "client alpha phrase");
        index.insert_content(second.id, "client beta phrase");

        let hits = index.query(r#""client alpha" OR "client beta""#, 10);

        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn filters_by_kind_extension_path_and_size() {
        let mut index = SearchIndex::new();
        let mut keep = record(1, "/Users/me/Desktop/PLAN.md", "PLAN.md");
        keep.len = 16 * 1024;
        let mut wrong_ext = record(2, "/Users/me/Desktop/PLAN.pdf", "PLAN.pdf");
        wrong_ext.len = 16 * 1024;
        let mut too_small = record(3, "/Users/me/Desktop/tiny.md", "tiny.md");
        too_small.len = 12;
        let mut folder = record(4, "/Users/me/Desktop/Docs", "Docs");
        folder.kind = FileKind::Directory;
        index.insert(keep);
        index.insert(wrong_ext);
        index.insert(too_small);
        index.insert(folder);

        let hits = index.query("kind:file ext:md path:desktop size:>1kb", 10);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record.name, "PLAN.md");
    }

    #[test]
    fn filters_by_modified_created_and_changed_dates() {
        let mut index = SearchIndex::new();
        let mut recent = record(1, "/tmp/recent.md", "recent.md");
        recent.modified = Some(test_time(2026, 8, 24));
        recent.created = Some(test_time(2026, 8, 1));
        recent.changed = Some(test_time(2026, 8, 24));
        let mut old = record(2, "/tmp/old.md", "old.md");
        old.modified = Some(test_time(2024, 1, 15));
        old.created = Some(test_time(2024, 1, 1));
        old.changed = Some(test_time(2024, 1, 15));
        index.insert(recent);
        index.insert(old);

        let modified = index.query("ext:md modified:>=2026-01-01", 10);
        let created = index.query("ext:md created:<2025-01-01", 10);
        let changed = index.query("ext:md changed:2026-08-24", 10);

        assert_eq!(modified.len(), 1);
        assert_eq!(modified[0].record.name, "recent.md");
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].record.name, "old.md");
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].record.name, "recent.md");
    }

    #[test]
    fn supports_negative_date_filters() {
        let mut index = SearchIndex::new();
        let mut recent = record(1, "/tmp/recent.md", "recent.md");
        recent.modified = Some(test_time(2026, 8, 24));
        let mut old = record(2, "/tmp/old.md", "old.md");
        old.modified = Some(test_time(2024, 1, 15));
        index.insert(recent);
        index.insert(old);

        let hits = index.query("ext:md -modified:>=2026-01-01", 10);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record.name, "old.md");
    }

    #[test]
    fn filters_without_terms_return_matching_records() {
        let mut index = SearchIndex::new();
        index.insert(record(1, "/tmp/notes.md", "notes.md"));
        index.insert(record(2, "/tmp/archive.pdf", "archive.pdf"));

        let hits = index.query("ext:md", 10);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record.name, "notes.md");
    }

    #[test]
    fn excludes_terms_and_matches_quoted_path_phrases() {
        let mut index = SearchIndex::new();
        index.insert(record(
            1,
            "/Users/me/Desktop/Client Work/final notes.md",
            "final notes.md",
        ));
        index.insert(record(
            2,
            "/Users/me/Desktop/Client Work/draft notes.md",
            "draft notes.md",
        ));

        let hits = index.query(r#""Client Work" notes -draft"#, 10);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record.name, "final notes.md");
    }

    #[test]
    fn supports_boolean_or_and_not_queries() {
        let mut index = SearchIndex::new();
        index.insert(record(1, "/tmp/report.md", "report.md"));
        index.insert(record(2, "/tmp/invoice.md", "invoice.md"));
        index.insert(record(3, "/tmp/draft-report.md", "draft-report.md"));
        index.insert(record(4, "/tmp/notes.md", "notes.md"));

        let hits = index.query("(report OR invoice) NOT draft", 10);
        let names: Vec<_> = hits.iter().map(|hit| hit.record.name.as_str()).collect();

        assert_eq!(names, vec!["invoice.md", "report.md"]);
    }

    #[test]
    fn supports_boolean_or_between_filters() {
        let mut index = SearchIndex::new();
        index.insert(record(1, "/tmp/report.md", "report.md"));
        index.insert(record(2, "/tmp/invoice.pdf", "invoice.pdf"));
        index.insert(record(3, "/tmp/image.png", "image.png"));

        let hits = index.query("ext:md OR ext:pdf", 10);
        let names: Vec<_> = hits.iter().map(|hit| hit.record.name.as_str()).collect();

        assert_eq!(names, vec!["invoice.pdf", "report.md"]);
    }

    #[test]
    fn searches_and_filters_finder_tags() {
        let mut index = SearchIndex::new();
        let mut keep = record(1, "/tmp/report.md", "report.md");
        keep.tags = vec!["Important".to_string(), "Client".to_string()];
        let mut skip = record(2, "/tmp/draft.md", "draft.md");
        skip.tags = vec!["Later".to_string()];
        index.insert(keep);
        index.insert(skip);

        let filtered = index.query("tag:important", 10);
        let plain = index.query("client", 10);

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].record.name, "report.md");
        assert_eq!(plain.len(), 1);
        assert_eq!(plain[0].reason, MatchReason::Tag);
    }

    #[test]
    fn removes_reindexed_tag_postings() {
        let mut index = SearchIndex::new();
        let mut item = record(1, "/tmp/report.md", "report.md");
        item.tags = vec!["Important".to_string()];
        index.insert(item.clone());
        item.tags = vec!["Later".to_string()];
        index.insert(item);

        assert!(index.query("tag:important", 10).is_empty());
        assert_eq!(index.query("tag:later", 10).len(), 1);
    }

    #[test]
    fn filters_named_and_absolute_scopes() {
        let mut index = SearchIndex::new();
        index.insert(record(1, "/Users/me/Desktop/report.md", "report.md"));
        index.insert(record(2, "/Users/me/Downloads/report.md", "report.md"));
        index.insert(record(3, "/Users/me/Documents/report.md", "report.md"));

        let desktop = index.query("report @desktop", 10);
        let downloads = index.query("report scope:downloads", 10);
        let subtree = index.query("report scope:/Users/me/Documents", 10);

        assert_eq!(desktop.len(), 1);
        assert!(desktop[0].record.path.ends_with("Desktop/report.md"));
        assert_eq!(downloads.len(), 1);
        assert!(downloads[0].record.path.ends_with("Downloads/report.md"));
        assert_eq!(subtree.len(), 1);
        assert!(subtree[0].record.path.ends_with("Documents/report.md"));
    }

    #[test]
    fn supports_negative_scope_filters() {
        let mut index = SearchIndex::new();
        index.insert(record(1, "/Users/me/Desktop/report.md", "report.md"));
        index.insert(record(2, "/Users/me/Downloads/report.md", "report.md"));

        let hits = index.query("report -@desktop", 10);

        assert_eq!(hits.len(), 1);
        assert!(hits[0].record.path.ends_with("Downloads/report.md"));
    }

    fn record(node: u64, path: &str, name: &str) -> FileRecord {
        FileRecord {
            id: FileId::new(VolumeId(1), node),
            parent: None,
            path: PathBuf::from(path),
            name: name.to_string(),
            kind: FileKind::File,
            len: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: Vec::new(),
        }
    }

    fn test_time(year: i32, month: u32, day: u32) -> std::time::SystemTime {
        UNIX_EPOCH + Duration::from_secs(test_days_from_civil(year, month, day) as u64 * 86_400)
    }

    fn test_days_from_civil(year: i32, month: u32, day: u32) -> i64 {
        let (year, month) = if month <= 2 {
            (year - 1, month + 12)
        } else {
            (year, month)
        };
        let era = if year >= 0 { year } else { year - 399 } / 400;
        let year_of_era = year - era * 400;
        let day_of_year = (153 * (month as i32 - 3) + 2) / 5 + day as i32 - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        era as i64 * 146_097 + day_of_era as i64 - 719_468
    }
}
