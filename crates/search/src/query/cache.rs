use super::{
    DateComparison, DateField, QueryExpr, QueryFilter, QueryKind, QueryProximity, QueryScope,
    SearchQuery, SizeComparison, SizeOperator,
};
use std::fmt::Write;

impl SearchQuery {
    pub fn canonical_cache_key(&self) -> String {
        let mut key = String::from("q1");
        push_strings(&mut key, "terms", &self.terms);
        push_strings(&mut key, "excluded", &self.excluded_terms);
        push_strings(&mut key, "phrases", &self.phrases);
        push_proximities(&mut key, "proximities", &self.proximities);
        push_filters(&mut key, "filters", &self.filters);
        match &self.expression {
            Some(expression) => {
                key.push_str("|expr:some");
                push_expression(&mut key, expression);
            }
            None => key.push_str("|expr:none"),
        }
        key
    }
}

fn push_strings(key: &mut String, label: &str, values: &[String]) {
    push_labelled_len(key, label, values.len());
    for value in values {
        push_string(key, value);
    }
}

fn push_proximities(key: &mut String, label: &str, values: &[QueryProximity]) {
    push_labelled_len(key, label, values.len());
    for proximity in values {
        push_proximity(key, proximity);
    }
}

fn push_filters(key: &mut String, label: &str, values: &[QueryFilter]) {
    push_labelled_len(key, label, values.len());
    for filter in values {
        push_filter(key, filter);
    }
}

fn push_labelled_len(key: &mut String, label: &str, len: usize) {
    key.push('|');
    key.push_str(label);
    key.push(':');
    let _ = write!(key, "{len}");
}

fn push_string(key: &mut String, value: &str) {
    let _ = write!(key, ":{}", value.len());
    key.push(':');
    key.push_str(value);
}

fn push_bool(key: &mut String, value: bool) {
    key.push(if value { '1' } else { '0' });
}

fn push_proximity(key: &mut String, proximity: &QueryProximity) {
    let _ = write!(key, ":near:{}", proximity.distance);
    push_strings(key, "terms", &proximity.terms);
}

fn push_expression(key: &mut String, expression: &QueryExpr) {
    match expression {
        QueryExpr::Term(value) => {
            key.push_str(":term");
            push_string(key, value);
        }
        QueryExpr::Phrase(value) => {
            key.push_str(":phrase");
            push_string(key, value);
        }
        QueryExpr::Proximity(proximity) => {
            key.push_str(":proximity");
            push_proximity(key, proximity);
        }
        QueryExpr::Filter(filter) => {
            key.push_str(":filter");
            push_filter(key, filter);
        }
        QueryExpr::Not(expression) => {
            key.push_str(":not");
            push_expression(key, expression);
        }
        QueryExpr::And(expressions) => {
            push_labelled_len(key, "and", expressions.len());
            for expression in expressions {
                push_expression(key, expression);
            }
        }
        QueryExpr::Or(expressions) => {
            push_labelled_len(key, "or", expressions.len());
            for expression in expressions {
                push_expression(key, expression);
            }
        }
    }
}

fn push_filter(key: &mut String, filter: &QueryFilter) {
    match filter {
        QueryFilter::Name(value, negative) => {
            key.push_str(":name:");
            push_bool(key, *negative);
            push_string(key, value);
        }
        QueryFilter::Path(value, negative) => {
            key.push_str(":path:");
            push_bool(key, *negative);
            push_string(key, value);
        }
        QueryFilter::Extension(value, negative) => {
            key.push_str(":extension:");
            push_bool(key, *negative);
            push_string(key, value);
        }
        QueryFilter::Tag(value, negative) => {
            key.push_str(":tag:");
            push_bool(key, *negative);
            push_string(key, value);
        }
        QueryFilter::Scope(scope, negative) => {
            key.push_str(":scope:");
            push_bool(key, *negative);
            push_scope(key, scope);
        }
        QueryFilter::Kind(kind, negative) => {
            key.push_str(":kind:");
            push_bool(key, *negative);
            push_kind(key, *kind);
        }
        QueryFilter::Size(size, negative) => {
            key.push_str(":size:");
            push_bool(key, *negative);
            push_size(key, *size);
        }
        QueryFilter::Date(field, date, negative) => {
            key.push_str(":date:");
            push_bool(key, *negative);
            push_date_field(key, *field);
            push_date(key, *date);
        }
    }
}

fn push_scope(key: &mut String, scope: &QueryScope) {
    match scope {
        QueryScope::Desktop => key.push_str(":desktop"),
        QueryScope::Documents => key.push_str(":documents"),
        QueryScope::Downloads => key.push_str(":downloads"),
        QueryScope::Applications => key.push_str(":applications"),
        QueryScope::ICloud => key.push_str(":icloud"),
        QueryScope::Trash => key.push_str(":trash"),
        QueryScope::Home => key.push_str(":home"),
        QueryScope::Path(path) => {
            key.push_str(":path");
            push_string(key, path);
        }
    }
}

fn push_kind(key: &mut String, kind: QueryKind) {
    key.push_str(match kind {
        QueryKind::Directory => ":directory",
        QueryKind::File => ":file",
        QueryKind::Symlink => ":symlink",
        QueryKind::Other => ":other",
    });
}

fn push_size(key: &mut String, size: SizeComparison) {
    push_size_operator(key, size.operator);
    let _ = write!(key, ":{}", size.bytes);
}

fn push_size_operator(key: &mut String, operator: SizeOperator) {
    key.push_str(match operator {
        SizeOperator::Eq => ":eq",
        SizeOperator::Gt => ":gt",
        SizeOperator::Gte => ":gte",
        SizeOperator::Lt => ":lt",
        SizeOperator::Lte => ":lte",
    });
}

fn push_date_field(key: &mut String, field: DateField) {
    key.push_str(match field {
        DateField::Created => ":created",
        DateField::Modified => ":modified",
        DateField::Changed => ":changed",
    });
}

fn push_date(key: &mut String, date: DateComparison) {
    match date {
        DateComparison::On { start, end } => {
            let _ = write!(key, ":on:{start}:{end}");
        }
        DateComparison::Before { end } => {
            let _ = write!(key, ":before:{end}");
        }
        DateComparison::BeforeOrOn { end } => {
            let _ = write!(key, ":before-or-on:{end}");
        }
        DateComparison::After { start } => {
            let _ = write!(key, ":after:{start}");
        }
        DateComparison::AfterOrOn { start } => {
            let _ = write!(key, ":after-or-on:{start}");
        }
    }
}
