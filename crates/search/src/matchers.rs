use super::columns::filter_matches_columns;
use super::fuzzy::bounded_levenshtein;
use super::intent::term_matches_intent;
use super::{QueryExpr, QueryFilter, SearchIndex, SearchPass, SearchQuery};
use gfm_jobs::Cancellation;
use gfm_types::FileRecord;

impl SearchIndex {
    pub(super) fn record_matches_query_cancellable(
        &self,
        record: &FileRecord,
        query: &SearchQuery,
        pass: SearchPass,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<bool> {
        cancellation.check()?;
        if let Some(expression) = &query.expression {
            return self.record_matches_expression_cancellable(
                record,
                expression,
                pass,
                cancellation,
            );
        }
        for term in &query.excluded_terms {
            cancellation.check()?;
            if self.record_contains_term(record, term)
                || (pass.includes_deep() && self.content_has(record.id, term))
            {
                return Ok(false);
            }
        }
        for phrase in &query.phrases {
            cancellation.check()?;
            if self.record_matches_phrase(record, phrase) {
                continue;
            }
            if pass.includes_deep()
                && self.content_matches_phrase_cancellable(record.id, phrase, cancellation)?
            {
                continue;
            }
            return Ok(false);
        }
        if pass.includes_deep() {
            for proximity in &query.proximities {
                cancellation.check()?;
                if !self.content_matches_proximity_cancellable(
                    record.id,
                    proximity,
                    cancellation,
                )? {
                    return Ok(false);
                }
            }
        }
        if !pass.includes_deep() && !query.proximities.is_empty() {
            return Ok(false);
        }
        for filter in &query.filters {
            cancellation.check()?;
            if !self.filter_matches(record, filter) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn record_matches_expression_cancellable(
        &self,
        record: &FileRecord,
        expression: &QueryExpr,
        pass: SearchPass,
        cancellation: &Cancellation,
    ) -> gfm_types::Result<bool> {
        cancellation.check()?;
        match expression {
            QueryExpr::Term(term) => Ok(self.record_contains_term(record, term)
                || (pass.includes_deep() && self.content_has(record.id, term))
                || (pass.includes_deep() && self.record_fuzzy_matches_term(record, term))
                || term_matches_intent(term, record)),
            QueryExpr::Phrase(phrase) => Ok(self.record_matches_phrase(record, phrase)
                || (pass.includes_deep()
                    && self.content_matches_phrase_cancellable(
                        record.id,
                        phrase,
                        cancellation,
                    )?)),
            QueryExpr::Proximity(proximity) => Ok(pass.includes_deep()
                && self.content_matches_proximity_cancellable(
                    record.id,
                    proximity,
                    cancellation,
                )?),
            QueryExpr::Filter(filter) => Ok(self.filter_matches(record, filter)),
            QueryExpr::Not(expression) => Ok(!self.record_matches_expression_cancellable(
                record,
                expression,
                pass,
                cancellation,
            )?),
            QueryExpr::And(expressions) => {
                for expression in expressions {
                    if !self.record_matches_expression_cancellable(
                        record,
                        expression,
                        pass,
                        cancellation,
                    )? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            QueryExpr::Or(expressions) => {
                for expression in expressions {
                    if self.record_matches_expression_cancellable(
                        record,
                        expression,
                        pass,
                        cancellation,
                    )? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }

    pub(super) fn record_contains_term(&self, record: &FileRecord, term: &str) -> bool {
        self.columns
            .get(&record.id)
            .is_some_and(|columns| columns.contains_term(term))
    }

    pub(super) fn record_matches_phrase(&self, record: &FileRecord, phrase: &str) -> bool {
        self.columns
            .get(&record.id)
            .is_some_and(|columns| columns.matches_phrase(phrase))
    }

    fn filter_matches(&self, record: &FileRecord, filter: &QueryFilter) -> bool {
        self.columns
            .get(&record.id)
            .is_some_and(|columns| filter_matches_columns(filter, record, columns))
    }

    pub(super) fn record_fuzzy_matches_term(&self, record: &FileRecord, term: &str) -> bool {
        self.columns.get(&record.id).is_some_and(|columns| {
            columns
                .fuzzy_terms
                .iter()
                .any(|candidate| bounded_levenshtein(candidate, term, 2).is_some())
        })
    }
}
