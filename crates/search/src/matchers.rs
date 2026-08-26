use super::columns::filter_matches_columns;
use super::fuzzy::bounded_levenshtein;
use super::intent::term_matches_intent;
use super::{QueryExpr, QueryFilter, SearchIndex, SearchPass, SearchQuery};
use gfm_types::FileRecord;

impl SearchIndex {
    pub(super) fn record_matches_query(
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
