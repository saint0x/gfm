use super::{
    rarest_term_postings, tokenize, QueryExpr, QueryFilter, QueryKind, RecordColumns, SearchIndex,
    SearchPass,
};
use gfm_jobs::Cancellation;
use gfm_types::{FileId, FileKind};
use std::collections::{BTreeMap, BTreeSet};

const CANCELLATION_STRIDE: usize = 256;

impl SearchIndex {
    pub(super) fn expression_candidate_ids(
        &self,
        expression: &QueryExpr,
        pass: SearchPass,
    ) -> Option<BTreeSet<FileId>> {
        self.expression_candidate_ids_cancellable(expression, pass, &Cancellation::default())
            .ok()
            .flatten()
    }

    pub(super) fn expression_candidate_ids_cancellable(
        &self,
        expression: &QueryExpr,
        pass: SearchPass,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<Option<BTreeSet<FileId>>> {
        cancellation.check()?;
        match expression {
            QueryExpr::Term(term) => Ok(Some(self.term_candidate_ids_cancellable(
                term,
                pass,
                cancellation,
            )?)),
            QueryExpr::Phrase(phrase) => {
                let mut ids = BTreeSet::new();
                extend_ids(
                    ids_from_record_phrase(self.record_phrase_ids(phrase)),
                    &mut ids,
                    cancellation,
                )?;
                if pass.includes_deep() {
                    extend_ids(
                        ids_from_record_phrase(self.content_phrase_ids(phrase)),
                        &mut ids,
                        cancellation,
                    )?;
                }
                Ok(Some(ids))
            }
            QueryExpr::Proximity(proximity) => pass
                .includes_deep()
                .then(|| {
                    collect_ids_cancellable(
                        self.content_proximity_ids(proximity).into_iter(),
                        cancellation,
                    )
                })
                .transpose(),
            QueryExpr::Filter(filter) => {
                self.filter_candidate_ids_cancellable(filter, cancellation)
            }
            QueryExpr::Not(_) => Ok(None),
            QueryExpr::And(expressions) => {
                self.and_expression_candidate_ids_cancellable(expressions, pass, cancellation)
            }
            QueryExpr::Or(expressions) => {
                if expressions.is_empty() {
                    return Ok(Some(BTreeSet::new()));
                }
                let mut ids = BTreeSet::new();
                for expression in expressions {
                    let Some(candidates) =
                        self.expression_candidate_ids_cancellable(expression, pass, cancellation)?
                    else {
                        return Ok(None);
                    };
                    extend_ids(candidates.into_iter(), &mut ids, cancellation)?;
                }
                Ok(Some(ids))
            }
        }
    }

    fn and_expression_candidate_ids_cancellable(
        &self,
        expressions: &[QueryExpr],
        pass: SearchPass,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<Option<BTreeSet<FileId>>> {
        let exact = expressions
            .iter()
            .map(|expression| {
                self.exact_expression_candidate_ids_cancellable(expression, pass, cancellation)
            })
            .collect::<gfm_types::Result<Vec<_>>>()?
            .into_iter()
            .flatten();

        if let Some(ids) = intersect_candidate_sets_cancellable(exact, cancellation)? {
            return Ok(Some(ids));
        }

        let mut smallest = None::<BTreeSet<FileId>>;
        for expression in expressions {
            cancellation.check()?;
            if let Some(ids) =
                self.expression_candidate_ids_cancellable(expression, pass, cancellation)?
            {
                if smallest
                    .as_ref()
                    .is_none_or(|current| ids.len() < current.len())
                {
                    smallest = Some(ids);
                }
            }
        }
        Ok(smallest)
    }

    pub(super) fn exact_expression_candidate_ids(
        &self,
        expression: &QueryExpr,
        pass: SearchPass,
    ) -> Option<BTreeSet<FileId>> {
        self.exact_expression_candidate_ids_cancellable(expression, pass, &Cancellation::default())
            .ok()
            .flatten()
    }

    pub(super) fn exact_expression_candidate_ids_cancellable(
        &self,
        expression: &QueryExpr,
        pass: SearchPass,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<Option<BTreeSet<FileId>>> {
        cancellation.check()?;
        match expression {
            QueryExpr::Phrase(phrase) => {
                let mut ids = BTreeSet::new();
                extend_ids(
                    ids_from_record_phrase(self.record_phrase_ids(phrase)),
                    &mut ids,
                    cancellation,
                )?;
                if pass.includes_deep() {
                    extend_ids(
                        ids_from_record_phrase(self.content_phrase_ids(phrase)),
                        &mut ids,
                        cancellation,
                    )?;
                }
                Ok(Some(ids))
            }
            QueryExpr::Proximity(proximity) => pass
                .includes_deep()
                .then(|| {
                    collect_ids_cancellable(
                        self.content_proximity_ids(proximity).into_iter(),
                        cancellation,
                    )
                })
                .transpose(),
            QueryExpr::Filter(filter) => {
                self.filter_candidate_ids_cancellable(filter, cancellation)
            }
            QueryExpr::And(expressions) => {
                self.exact_and_expression_candidate_ids_cancellable(expressions, pass, cancellation)
            }
            QueryExpr::Or(expressions) => {
                if expressions.is_empty() {
                    return Ok(Some(BTreeSet::new()));
                }
                let mut ids = BTreeSet::new();
                for expression in expressions {
                    let Some(candidates) = self.exact_expression_candidate_ids_cancellable(
                        expression,
                        pass,
                        cancellation,
                    )?
                    else {
                        return Ok(None);
                    };
                    extend_ids(candidates.into_iter(), &mut ids, cancellation)?;
                }
                Ok(Some(ids))
            }
            QueryExpr::Term(_) | QueryExpr::Not(_) => Ok(None),
        }
    }

    fn exact_and_expression_candidate_ids_cancellable(
        &self,
        expressions: &[QueryExpr],
        pass: SearchPass,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<Option<BTreeSet<FileId>>> {
        let mut candidate_sets = Vec::with_capacity(expressions.len());
        for expression in expressions {
            cancellation.check()?;
            let Some(candidates) =
                self.exact_expression_candidate_ids_cancellable(expression, pass, cancellation)?
            else {
                return Ok(None);
            };
            candidate_sets.push(candidates);
        }
        Ok(
            intersect_candidate_sets_cancellable(candidate_sets, cancellation)?
                .or_else(|| Some(BTreeSet::new())),
        )
    }

    pub(super) fn term_candidate_ids(&self, term: &str, pass: SearchPass) -> BTreeSet<FileId> {
        self.term_candidate_ids_cancellable(term, pass, &Cancellation::default())
            .unwrap_or_default()
    }

    pub(super) fn term_candidate_ids_cancellable(
        &self,
        term: &str,
        pass: SearchPass,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<BTreeSet<FileId>> {
        cancellation.check()?;
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
            extend_ids(postings.iter().copied(), &mut ids, cancellation)?;
        }
        if pass.includes_deep() {
            if let Some(postings) = self.content_terms.get(term) {
                extend_ids(postings.keys().copied(), &mut ids, cancellation)?;
            }
        }
        Ok(ids)
    }

    fn filter_candidate_ids_cancellable(
        &self,
        filter: &QueryFilter,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<Option<BTreeSet<FileId>>> {
        cancellation.check()?;
        match filter {
            QueryFilter::Name(value, false) => Ok(self.text_filter_candidate_ids_cancellable(
                value,
                &self.name_terms,
                |columns, value| columns.name.contains(value),
                cancellation,
            )?),
            QueryFilter::Path(value, false) => Ok(self.text_filter_candidate_ids_cancellable(
                value,
                &self.path_terms,
                |columns, value| columns.path.contains(value),
                cancellation,
            )?),
            QueryFilter::Extension(value, false) => {
                Ok(Some(self.extension.get(value).cloned().unwrap_or_default()))
            }
            QueryFilter::Tag(value, false) => {
                Ok(Some(self.tags.get(value).cloned().unwrap_or_default()))
            }
            QueryFilter::Kind(kind, false) => Ok(Some(
                self.kind
                    .get(&query_kind_file_kind(*kind))
                    .cloned()
                    .unwrap_or_default(),
            )),
            QueryFilter::Scope(_, false)
            | QueryFilter::Size(_, false)
            | QueryFilter::Date(_, _, false) => Ok(None),
            QueryFilter::Name(_, true)
            | QueryFilter::Path(_, true)
            | QueryFilter::Extension(_, true)
            | QueryFilter::Tag(_, true)
            | QueryFilter::Scope(_, true)
            | QueryFilter::Kind(_, true)
            | QueryFilter::Size(_, true)
            | QueryFilter::Date(_, _, true) => Ok(None),
        }
    }

    fn text_filter_candidate_ids_cancellable(
        &self,
        value: &str,
        postings: &BTreeMap<String, BTreeSet<FileId>>,
        matches: impl Fn(&RecordColumns, &str) -> bool,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<Option<BTreeSet<FileId>>> {
        cancellation.check()?;
        let terms = tokenize(value);
        let Some(candidates) = rarest_term_postings(&terms, postings) else {
            return Ok(None);
        };
        let mut ids = BTreeSet::new();
        for (index, id) in candidates.iter().copied().enumerate() {
            if index % CANCELLATION_STRIDE == 0 {
                cancellation.check()?;
            }
            if self
                .columns
                .get(&id)
                .is_some_and(|columns| matches(columns, value))
            {
                ids.insert(id);
            }
        }
        Ok(Some(ids))
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
    intersect_candidate_sets_cancellable(candidate_sets, &Cancellation::default())
        .ok()
        .flatten()
}

pub(super) fn intersect_candidate_sets_cancellable<I>(
    candidate_sets: I,
    cancellation: &Cancellation,
) -> gfm_types::Result<Option<BTreeSet<FileId>>>
where
    I: IntoIterator<Item = BTreeSet<FileId>>,
{
    cancellation.check()?;
    let mut candidate_sets = candidate_sets.into_iter().collect::<Vec<_>>();
    candidate_sets.sort_by_key(BTreeSet::len);

    let mut sets = candidate_sets.into_iter();
    let Some(mut ids) = sets.next() else {
        return Ok(None);
    };
    for (set_index, candidates) in sets.enumerate() {
        if set_index % CANCELLATION_STRIDE == 0 {
            cancellation.check()?;
        }
        let mut retained = BTreeSet::new();
        for (id_index, id) in ids.iter().copied().enumerate() {
            if id_index % CANCELLATION_STRIDE == 0 {
                cancellation.check()?;
            }
            if candidates.contains(&id) {
                retained.insert(id);
            }
        }
        ids = retained;
        if ids.is_empty() {
            break;
        }
    }
    Ok(Some(ids))
}

fn collect_ids_cancellable<I>(
    ids: I,
    cancellation: &Cancellation,
) -> gfm_types::Result<BTreeSet<FileId>>
where
    I: IntoIterator<Item = FileId>,
{
    let mut collected = BTreeSet::new();
    extend_ids(ids, &mut collected, cancellation)?;
    Ok(collected)
}

fn extend_ids<I>(
    ids: I,
    collected: &mut BTreeSet<FileId>,
    cancellation: &Cancellation,
) -> gfm_types::Result<()>
where
    I: IntoIterator<Item = FileId>,
{
    for (index, id) in ids.into_iter().enumerate() {
        if index % CANCELLATION_STRIDE == 0 {
            cancellation.check()?;
        }
        collected.insert(id);
    }
    Ok(())
}

fn ids_from_record_phrase(ids: Vec<FileId>) -> impl Iterator<Item = FileId> {
    ids.into_iter()
}

fn query_kind_file_kind(kind: QueryKind) -> FileKind {
    match kind {
        QueryKind::Directory => FileKind::Directory,
        QueryKind::File => FileKind::File,
        QueryKind::Symlink => FileKind::Symlink,
        QueryKind::Other => FileKind::Other,
    }
}
