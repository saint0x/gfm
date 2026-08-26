use super::{
    rarest_term_postings, tokenize, QueryExpr, QueryFilter, QueryKind, RecordColumns, SearchIndex,
    SearchPass,
};
use gfm_types::{FileId, FileKind};
use std::collections::{BTreeMap, BTreeSet};

impl SearchIndex {
    pub(super) fn expression_candidate_ids(
        &self,
        expression: &QueryExpr,
        pass: SearchPass,
    ) -> Option<BTreeSet<FileId>> {
        match expression {
            QueryExpr::Term(term) => Some(self.term_candidate_ids(term, pass)),
            QueryExpr::Phrase(phrase) => Some(
                self.record_phrase_ids(phrase)
                    .into_iter()
                    .chain(
                        pass.includes_deep()
                            .then(|| self.content_phrase_ids(phrase))
                            .into_iter()
                            .flatten(),
                    )
                    .collect(),
            ),
            QueryExpr::Proximity(proximity) => pass
                .includes_deep()
                .then(|| self.content_proximity_ids(proximity).into_iter().collect()),
            QueryExpr::Filter(filter) => self.filter_candidate_ids(filter),
            QueryExpr::Not(_) => None,
            QueryExpr::And(expressions) => self.and_expression_candidate_ids(expressions, pass),
            QueryExpr::Or(expressions) => {
                if expressions.is_empty() {
                    return Some(BTreeSet::new());
                }
                let mut ids = BTreeSet::new();
                for expression in expressions {
                    ids.extend(self.expression_candidate_ids(expression, pass)?);
                }
                Some(ids)
            }
        }
    }

    fn and_expression_candidate_ids(
        &self,
        expressions: &[QueryExpr],
        pass: SearchPass,
    ) -> Option<BTreeSet<FileId>> {
        let exact = expressions
            .iter()
            .filter_map(|expression| self.exact_expression_candidate_ids(expression, pass));

        if let Some(ids) = intersect_candidate_sets(exact) {
            return Some(ids);
        }

        expressions
            .iter()
            .filter_map(|expression| self.expression_candidate_ids(expression, pass))
            .min_by_key(BTreeSet::len)
    }

    pub(super) fn exact_expression_candidate_ids(
        &self,
        expression: &QueryExpr,
        pass: SearchPass,
    ) -> Option<BTreeSet<FileId>> {
        match expression {
            QueryExpr::Phrase(phrase) => Some(
                self.record_phrase_ids(phrase)
                    .into_iter()
                    .chain(
                        pass.includes_deep()
                            .then(|| self.content_phrase_ids(phrase))
                            .into_iter()
                            .flatten(),
                    )
                    .collect(),
            ),
            QueryExpr::Proximity(proximity) => pass
                .includes_deep()
                .then(|| self.content_proximity_ids(proximity).into_iter().collect()),
            QueryExpr::Filter(filter) => self.filter_candidate_ids(filter),
            QueryExpr::And(expressions) => {
                self.exact_and_expression_candidate_ids(expressions, pass)
            }
            QueryExpr::Or(expressions) => {
                if expressions.is_empty() {
                    return Some(BTreeSet::new());
                }
                let mut ids = BTreeSet::new();
                for expression in expressions {
                    ids.extend(self.exact_expression_candidate_ids(expression, pass)?);
                }
                Some(ids)
            }
            QueryExpr::Term(_) | QueryExpr::Not(_) => None,
        }
    }

    fn exact_and_expression_candidate_ids(
        &self,
        expressions: &[QueryExpr],
        pass: SearchPass,
    ) -> Option<BTreeSet<FileId>> {
        intersect_candidate_sets(
            expressions
                .iter()
                .map(|expression| self.exact_expression_candidate_ids(expression, pass))
                .collect::<Option<Vec<_>>>()?,
        )
        .or_else(|| Some(BTreeSet::new()))
    }

    pub(super) fn term_candidate_ids(&self, term: &str, pass: SearchPass) -> BTreeSet<FileId> {
        let mut ids = BTreeSet::new();
        for postings in [
            self.name_terms.get(term),
            self.path_terms.get(term),
            self.metadata_terms.get(term),
            self.extension.get(term),
            self.tags.get(term),
        ]
        .into_iter()
        .flatten()
        {
            ids.extend(postings);
        }
        if pass.includes_deep() {
            if let Some(postings) = self.content_terms.get(term) {
                ids.extend(postings.keys());
            }
        }
        ids
    }

    fn filter_candidate_ids(&self, filter: &QueryFilter) -> Option<BTreeSet<FileId>> {
        match filter {
            QueryFilter::Name(value, false) => {
                self.text_filter_candidate_ids(value, &self.name_terms, |columns, value| {
                    columns.name.contains(value)
                })
            }
            QueryFilter::Path(value, false) => {
                self.text_filter_candidate_ids(value, &self.path_terms, |columns, value| {
                    columns.path.contains(value)
                })
            }
            QueryFilter::Extension(value, false) => {
                Some(self.extension.get(value).cloned().unwrap_or_default())
            }
            QueryFilter::Tag(value, false) => {
                Some(self.tags.get(value).cloned().unwrap_or_default())
            }
            QueryFilter::Kind(kind, false) => Some(
                self.kind
                    .get(&query_kind_file_kind(*kind))
                    .cloned()
                    .unwrap_or_default(),
            ),
            QueryFilter::Scope(_, false)
            | QueryFilter::Size(_, false)
            | QueryFilter::Date(_, _, false) => None,
            QueryFilter::Name(_, true)
            | QueryFilter::Path(_, true)
            | QueryFilter::Extension(_, true)
            | QueryFilter::Tag(_, true)
            | QueryFilter::Scope(_, true)
            | QueryFilter::Kind(_, true)
            | QueryFilter::Size(_, true)
            | QueryFilter::Date(_, _, true) => None,
        }
    }

    fn text_filter_candidate_ids(
        &self,
        value: &str,
        postings: &BTreeMap<String, BTreeSet<FileId>>,
        matches: impl Fn(&RecordColumns, &str) -> bool,
    ) -> Option<BTreeSet<FileId>> {
        let terms = tokenize(value);
        let candidates = rarest_term_postings(&terms, postings)?;
        Some(
            candidates
                .iter()
                .copied()
                .filter(|id| {
                    self.columns
                        .get(id)
                        .is_some_and(|columns| matches(columns, value))
                })
                .collect(),
        )
    }
}

pub(super) fn expression_needs_universe(expression: &QueryExpr) -> bool {
    match expression {
        QueryExpr::Filter(_) | QueryExpr::Not(_) => true,
        QueryExpr::And(expressions) => {
            !expression_has_positive_anchor(expression)
                || expressions.iter().all(expression_needs_universe)
        }
        QueryExpr::Or(expressions) => expressions.iter().any(expression_needs_universe),
        QueryExpr::Term(_) | QueryExpr::Phrase(_) | QueryExpr::Proximity(_) => false,
    }
}

pub(super) fn expression_has_positive_anchor(expression: &QueryExpr) -> bool {
    match expression {
        QueryExpr::Term(_) | QueryExpr::Phrase(_) | QueryExpr::Proximity(_) => true,
        QueryExpr::Filter(_) | QueryExpr::Not(_) => false,
        QueryExpr::And(expressions) => expressions.iter().any(expression_has_positive_anchor),
        QueryExpr::Or(expressions) => {
            !expressions.is_empty() && expressions.iter().all(expression_has_positive_anchor)
        }
    }
}

pub(super) fn intersect_candidate_sets<I>(candidate_sets: I) -> Option<BTreeSet<FileId>>
where
    I: IntoIterator<Item = BTreeSet<FileId>>,
{
    let mut candidate_sets = candidate_sets.into_iter().collect::<Vec<_>>();
    candidate_sets.sort_by_key(BTreeSet::len);

    let mut sets = candidate_sets.into_iter();
    let mut ids = sets.next()?;
    for candidates in sets {
        ids.retain(|id| candidates.contains(id));
        if ids.is_empty() {
            break;
        }
    }
    Some(ids)
}

fn query_kind_file_kind(kind: QueryKind) -> FileKind {
    match kind {
        QueryKind::Directory => FileKind::Directory,
        QueryKind::File => FileKind::File,
        QueryKind::Symlink => FileKind::Symlink,
        QueryKind::Other => FileKind::Other,
    }
}
