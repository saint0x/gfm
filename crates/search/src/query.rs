use gfm_types::{FileKind, FileRecord};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchQuery {
    pub terms: Vec<String>,
    pub excluded_terms: Vec<String>,
    pub phrases: Vec<String>,
    pub proximities: Vec<QueryProximity>,
    pub filters: Vec<QueryFilter>,
    pub expression: Option<QueryExpr>,
}

impl SearchQuery {
    pub fn parse(input: &str) -> Self {
        let mut query = Self::default();
        let tokens = scan_query(input);
        query.expression = QueryParser::new(tokens.clone()).parse();
        for token in tokens {
            let Some(value) = token.as_value() else {
                continue;
            };
            if is_operator(value) {
                continue;
            }
            let negative = token.starts_with('-') && token.len() > 1;
            let value = if negative { &value[1..] } else { value };
            if let Some(proximity) = QueryProximity::parse(value) {
                if !negative {
                    query.proximities.push(proximity);
                }
                continue;
            }
            if let Some(filter) = QueryFilter::parse(value, negative) {
                query.filters.push(filter);
                continue;
            }

            if token.quoted {
                let phrase = normalize(value);
                if !phrase.is_empty() {
                    if negative {
                        query.excluded_terms.extend(tokenize(&phrase));
                    } else {
                        query.phrases.push(phrase);
                    }
                }
                continue;
            }

            let terms = tokenize(&normalize(value));
            if negative {
                query.excluded_terms.extend(terms);
            } else {
                query.terms.extend(terms);
            }
        }
        query.dedupe();
        query
    }

    fn dedupe(&mut self) {
        self.terms.sort();
        self.terms.dedup();
        self.excluded_terms.sort();
        self.excluded_terms.dedup();
        self.phrases.sort();
        self.phrases.dedup();
        self.proximities.sort();
        self.proximities.dedup();
        self.filters.sort();
        self.filters.dedup();
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.terms.is_empty()
            && self.excluded_terms.is_empty()
            && self.phrases.is_empty()
            && self.proximities.is_empty()
            && self.filters.is_empty()
            && self.expression.is_none()
    }

    pub fn content_candidate_terms(&self) -> Vec<String> {
        let mut terms = self.terms.clone();
        for phrase in &self.phrases {
            terms.extend(tokenize(phrase));
        }
        for proximity in &self.proximities {
            terms.extend(proximity.terms.iter().cloned());
        }
        terms.sort();
        terms.dedup();
        terms
    }

    pub fn comment_candidate_terms(&self) -> Vec<String> {
        self.content_candidate_terms()
    }

    pub fn tag_candidate_terms(&self) -> Vec<String> {
        let mut terms = self
            .filters
            .iter()
            .filter_map(|filter| match filter {
                QueryFilter::Tag(term, false) => Some(term.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        terms.sort();
        terms.dedup();
        terms
    }

    pub fn prefix_candidate_terms(&self) -> Vec<String> {
        let mut terms = self
            .content_candidate_terms()
            .into_iter()
            .filter(|term| crate::is_prefix_term(term))
            .collect::<Vec<_>>();
        terms.sort();
        terms.dedup();
        terms
    }

    pub fn fuzzy_candidate_keys(&self) -> Vec<String> {
        let mut keys = Vec::new();
        for term in self
            .content_candidate_terms()
            .into_iter()
            .filter(|term| crate::is_fuzzy_term(term))
        {
            keys.extend(crate::deletion_keys(&term, 2));
        }
        keys.sort();
        keys.dedup();
        keys
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryExpr {
    Term(String),
    Phrase(String),
    Proximity(QueryProximity),
    Filter(QueryFilter),
    Not(Box<QueryExpr>),
    And(Vec<QueryExpr>),
    Or(Vec<QueryExpr>),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct QueryProximity {
    pub distance: u32,
    pub terms: Vec<String>,
}

impl QueryProximity {
    fn parse(input: &str) -> Option<Self> {
        let rest = input
            .strip_prefix("near:")
            .or_else(|| input.strip_prefix("proximity:"))?;
        let (distance, terms) = rest.split_once(':')?;
        let distance: u32 = distance.trim().parse().ok()?;
        if distance == 0 || distance > 256 {
            return None;
        }
        let mut terms = tokenize(&normalize(terms));
        terms.sort();
        terms.dedup();
        (terms.len() >= 2).then_some(Self { distance, terms })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum QueryFilter {
    Name(String, bool),
    Path(String, bool),
    Extension(String, bool),
    Tag(String, bool),
    Scope(QueryScope, bool),
    Kind(QueryKind, bool),
    Size(SizeComparison, bool),
    Date(DateField, DateComparison, bool),
}

impl QueryFilter {
    fn parse(input: &str, negative: bool) -> Option<Self> {
        if let Some(scope) = input.strip_prefix('@').and_then(QueryScope::parse) {
            return Some(Self::Scope(scope, negative));
        }
        let (field, value) = input.split_once(':')?;
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        match field.trim().to_ascii_lowercase().as_str() {
            "name" => Some(Self::Name(normalize(value), negative)),
            "path" | "in" => Some(Self::Path(normalize(value), negative)),
            "ext" | "extension" => Some(Self::Extension(normalize_extension(value), negative)),
            "tag" | "label" => Some(Self::Tag(normalize(value), negative)),
            "scope" | "where" => QueryScope::parse(value).map(|scope| Self::Scope(scope, negative)),
            "kind" | "type" => QueryKind::parse(value).map(|kind| Self::Kind(kind, negative)),
            "size" => SizeComparison::parse(value).map(|size| Self::Size(size, negative)),
            "date" | "modified" | "mtime" => DateComparison::parse(value)
                .map(|date| Self::Date(DateField::Modified, date, negative)),
            "created" | "birth" | "btime" => DateComparison::parse(value)
                .map(|date| Self::Date(DateField::Created, date, negative)),
            "changed" | "ctime" => DateComparison::parse(value)
                .map(|date| Self::Date(DateField::Changed, date, negative)),
            _ => None,
        }
    }

    pub(crate) fn matches(&self, record: &FileRecord) -> bool {
        let positive = match self {
            Self::Name(value, _) => normalize(&record.name).contains(value),
            Self::Path(value, _) => normalize_path(&record.path).contains(value),
            Self::Extension(value, _) => record
                .extension()
                .map(normalize_extension)
                .is_some_and(|extension| extension == *value),
            Self::Tag(value, _) => record.tags.iter().any(|tag| normalize(tag) == *value),
            Self::Scope(scope, _) => scope.matches(record),
            Self::Kind(kind, _) => kind.matches_kind(record.kind),
            Self::Size(size, _) => size.matches(record.len),
            Self::Date(field, date, _) => field.time(record).is_some_and(|time| date.matches(time)),
        };
        if self.is_negative() {
            !positive
        } else {
            positive
        }
    }

    fn is_negative(&self) -> bool {
        match self {
            Self::Name(_, negative)
            | Self::Path(_, negative)
            | Self::Extension(_, negative)
            | Self::Tag(_, negative)
            | Self::Scope(_, negative)
            | Self::Kind(_, negative)
            | Self::Size(_, negative)
            | Self::Date(_, _, negative) => *negative,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum QueryKind {
    Directory,
    File,
    Symlink,
    Other,
}

impl QueryKind {
    fn parse(input: &str) -> Option<Self> {
        match normalize(input).as_str() {
            "dir" | "directory" | "folder" => Some(Self::Directory),
            "file" | "document" => Some(Self::File),
            "link" | "symlink" | "alias" => Some(Self::Symlink),
            "other" => Some(Self::Other),
            _ => None,
        }
    }

    pub(crate) fn matches_kind(self, kind: FileKind) -> bool {
        matches!(
            (self, kind),
            (Self::Directory, FileKind::Directory)
                | (Self::File, FileKind::File)
                | (Self::Symlink, FileKind::Symlink)
                | (Self::Other, FileKind::Other)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum QueryScope {
    Desktop,
    Documents,
    Downloads,
    Applications,
    ICloud,
    Trash,
    Home,
    Path(String),
}

impl QueryScope {
    fn parse(input: &str) -> Option<Self> {
        let value = input.trim();
        if value.is_empty() {
            return None;
        }
        let normalized = normalize(value);
        Some(match normalized.as_str() {
            "desktop" => Self::Desktop,
            "documents" | "docs" => Self::Documents,
            "downloads" => Self::Downloads,
            "applications" | "apps" => Self::Applications,
            "icloud" | "icloud-drive" | "icloud drive" => Self::ICloud,
            "trash" => Self::Trash,
            "home" => Self::Home,
            _ => Self::Path(normalized),
        })
    }

    fn matches(&self, record: &FileRecord) -> bool {
        match self {
            Self::Desktop => path_has_component(&record.path, "desktop"),
            Self::Documents => path_has_component(&record.path, "documents"),
            Self::Downloads => path_has_component(&record.path, "downloads"),
            Self::Applications => path_has_component(&record.path, "applications"),
            Self::ICloud => {
                path_contains_component_sequence(&record.path, &["mobile documents"])
                    || path_contains_component_sequence(&record.path, &["icloud drive"])
            }
            Self::Trash => {
                path_has_component(&record.path, ".trash")
                    || path_has_component(&record.path, "trash")
            }
            Self::Home => path_has_users_home_prefix(&record.path),
            Self::Path(path) => {
                let record_path = normalize_path(&record.path);
                record_path == *path || record_path.starts_with(&format!("{path}/"))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SizeOperator {
    Eq,
    Gt,
    Gte,
    Lt,
    Lte,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SizeComparison {
    operator: SizeOperator,
    bytes: u64,
}

impl SizeComparison {
    fn parse(input: &str) -> Option<Self> {
        let input = input.trim();
        let (operator, value) = parse_operator(input);
        parse_size(value).map(|bytes| Self { operator, bytes })
    }

    fn matches(self, len: u64) -> bool {
        match self.operator {
            SizeOperator::Eq => len == self.bytes,
            SizeOperator::Gt => len > self.bytes,
            SizeOperator::Gte => len >= self.bytes,
            SizeOperator::Lt => len < self.bytes,
            SizeOperator::Lte => len <= self.bytes,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DateField {
    Created,
    Modified,
    Changed,
}

impl DateField {
    fn time(self, record: &FileRecord) -> Option<SystemTime> {
        match self {
            Self::Created => record.created,
            Self::Modified => record.modified,
            Self::Changed => record.changed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DateComparison {
    On { start: u64, end: u64 },
    Before { end: u64 },
    BeforeOrOn { end: u64 },
    After { start: u64 },
    AfterOrOn { start: u64 },
}

impl DateComparison {
    fn parse(input: &str) -> Option<Self> {
        let input = input.trim();
        let (operator, value) = parse_operator(input);
        let (start, end) = date_bounds(value.trim())?;
        Some(match operator {
            SizeOperator::Eq => Self::On { start, end },
            SizeOperator::Gt => Self::After {
                start: end.saturating_add(1),
            },
            SizeOperator::Gte => Self::AfterOrOn { start },
            SizeOperator::Lt => Self::Before { end: start },
            SizeOperator::Lte => Self::BeforeOrOn { end },
        })
    }

    fn matches(self, time: SystemTime) -> bool {
        let Some(seconds) = time
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
        else {
            return false;
        };
        match self {
            Self::On { start, end } => (start..=end).contains(&seconds),
            Self::Before { end } => seconds < end,
            Self::BeforeOrOn { end } => seconds <= end,
            Self::After { start } => seconds >= start,
            Self::AfterOrOn { start } => seconds >= start,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum QueryTokenKind {
    Value,
    Open,
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QueryToken {
    value: String,
    quoted: bool,
    kind: QueryTokenKind,
}

impl QueryToken {
    fn starts_with(&self, prefix: char) -> bool {
        self.value.starts_with(prefix)
    }

    fn len(&self) -> usize {
        self.value.len()
    }

    fn as_str(&self) -> &str {
        &self.value
    }

    fn as_value(&self) -> Option<&str> {
        (self.kind == QueryTokenKind::Value).then_some(self.value.as_str())
    }
}

struct QueryParser {
    tokens: Vec<QueryToken>,
    offset: usize,
}

impl QueryParser {
    fn new(tokens: Vec<QueryToken>) -> Self {
        Self { tokens, offset: 0 }
    }

    fn parse(mut self) -> Option<QueryExpr> {
        let expression = self.parse_or()?;
        (self.offset == self.tokens.len()).then_some(expression)
    }

    fn parse_or(&mut self) -> Option<QueryExpr> {
        let mut expressions = vec![self.parse_and()?];
        while self.consume_operator("or") {
            expressions.push(self.parse_and()?);
        }
        Some(flatten_or(expressions))
    }

    fn parse_and(&mut self) -> Option<QueryExpr> {
        let mut expressions = Vec::new();
        while self.peek_value().is_some() || self.peek_kind(QueryTokenKind::Open) {
            if self.peek_operator("or") || self.peek_kind(QueryTokenKind::Close) {
                break;
            }
            self.consume_operator("and");
            expressions.push(self.parse_not()?);
        }
        (!expressions.is_empty()).then(|| flatten_and(expressions))
    }

    fn parse_not(&mut self) -> Option<QueryExpr> {
        if self.consume_operator("not") {
            return self.parse_not().map(|expr| QueryExpr::Not(Box::new(expr)));
        }
        let token = self.peek()?.clone();
        if token.kind == QueryTokenKind::Value
            && token.value.starts_with('-')
            && token.value.len() > 1
        {
            self.offset += 1;
            return atom_expr(&token.value[1..], token.quoted)
                .map(|expr| QueryExpr::Not(Box::new(expr)));
        }
        self.parse_atom()
    }

    fn parse_atom(&mut self) -> Option<QueryExpr> {
        if self.consume_kind(QueryTokenKind::Open) {
            let expression = self.parse_or()?;
            self.consume_kind(QueryTokenKind::Close)
                .then_some(expression)
        } else {
            let token = self.next_value()?;
            atom_expr(token.as_str(), token.quoted)
        }
    }

    fn peek(&self) -> Option<&QueryToken> {
        self.tokens.get(self.offset)
    }

    fn next_value(&mut self) -> Option<QueryToken> {
        let token = self.peek()?.clone();
        if token.kind == QueryTokenKind::Value {
            self.offset += 1;
            Some(token)
        } else {
            None
        }
    }

    fn peek_value(&self) -> Option<&str> {
        self.peek().and_then(QueryToken::as_value)
    }

    fn peek_operator(&self, operator: &str) -> bool {
        self.peek_value()
            .is_some_and(|value| value.eq_ignore_ascii_case(operator))
    }

    fn consume_operator(&mut self, operator: &str) -> bool {
        if self.peek_operator(operator) {
            self.offset += 1;
            true
        } else {
            false
        }
    }

    fn peek_kind(&self, kind: QueryTokenKind) -> bool {
        self.peek().is_some_and(|token| token.kind == kind)
    }

    fn consume_kind(&mut self, kind: QueryTokenKind) -> bool {
        if self.peek_kind(kind) {
            self.offset += 1;
            true
        } else {
            false
        }
    }
}

fn atom_expr(value: &str, quoted: bool) -> Option<QueryExpr> {
    if let Some(proximity) = QueryProximity::parse(value) {
        return Some(QueryExpr::Proximity(proximity));
    }
    if let Some(filter) = QueryFilter::parse(value, false) {
        return Some(QueryExpr::Filter(filter));
    }
    let value = normalize(value);
    if value.is_empty() {
        None
    } else if quoted {
        Some(QueryExpr::Phrase(value))
    } else {
        let terms = tokenize(&value).into_iter().map(QueryExpr::Term).collect();
        Some(flatten_and(terms))
    }
}

fn is_operator(value: &str) -> bool {
    value.eq_ignore_ascii_case("and")
        || value.eq_ignore_ascii_case("or")
        || value.eq_ignore_ascii_case("not")
}

fn flatten_and(expressions: Vec<QueryExpr>) -> QueryExpr {
    let mut flattened = Vec::new();
    for expression in expressions {
        match expression {
            QueryExpr::And(expressions) => flattened.extend(expressions),
            other => flattened.push(other),
        }
    }
    if flattened.len() == 1 {
        flattened.remove(0)
    } else {
        QueryExpr::And(flattened)
    }
}

fn flatten_or(expressions: Vec<QueryExpr>) -> QueryExpr {
    let mut flattened = Vec::new();
    for expression in expressions {
        match expression {
            QueryExpr::Or(expressions) => flattened.extend(expressions),
            other => flattened.push(other),
        }
    }
    if flattened.len() == 1 {
        flattened.remove(0)
    } else {
        QueryExpr::Or(flattened)
    }
}

pub(crate) fn normalize(input: &str) -> String {
    input.trim().to_lowercase()
}

pub(crate) fn tokenize(input: &str) -> Vec<String> {
    input
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn normalize_extension(input: &str) -> String {
    normalize(input).trim_start_matches('.').to_string()
}

fn normalize_path(path: &Path) -> String {
    normalize(&path.to_string_lossy())
}

fn path_has_component(path: &Path, expected: &str) -> bool {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .any(|component| normalize(component) == expected)
}

fn path_contains_component_sequence(path: &Path, expected: &[&str]) -> bool {
    let components: Vec<_> = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .map(normalize)
        .collect();
    if expected.is_empty() {
        return true;
    }
    components.windows(expected.len()).any(|window| {
        window
            .iter()
            .zip(expected)
            .all(|(left, right)| left == right)
    })
}

fn path_has_users_home_prefix(path: &Path) -> bool {
    let components: Vec<_> = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .map(normalize)
        .collect();
    components
        .windows(2)
        .any(|window| window[0] == "users" && !window[1].is_empty())
}

fn parse_operator(input: &str) -> (SizeOperator, &str) {
    if let Some(value) = input.strip_prefix(">=") {
        (SizeOperator::Gte, value)
    } else if let Some(value) = input.strip_prefix("<=") {
        (SizeOperator::Lte, value)
    } else if let Some(value) = input.strip_prefix('>') {
        (SizeOperator::Gt, value)
    } else if let Some(value) = input.strip_prefix('<') {
        (SizeOperator::Lt, value)
    } else if let Some(value) = input.strip_prefix('=') {
        (SizeOperator::Eq, value)
    } else {
        (SizeOperator::Eq, input)
    }
}

fn parse_size(input: &str) -> Option<u64> {
    let input = input.trim().to_ascii_lowercase();
    let split = input
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(input.len());
    let number: u64 = input[..split].parse().ok()?;
    let unit = input[split..].trim();
    let multiplier = match unit {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1024,
        "m" | "mb" | "mib" => 1024 * 1024,
        "g" | "gb" | "gib" => 1024 * 1024 * 1024,
        "t" | "tb" | "tib" => 1024_u64.pow(4),
        _ => return None,
    };
    number.checked_mul(multiplier)
}

fn date_bounds(input: &str) -> Option<(u64, u64)> {
    let (year, month, day) = parse_date(input)?;
    let start_days = days_from_civil(year, month, day)?;
    if start_days < 0 {
        return None;
    }
    let start = (start_days as u64).checked_mul(86_400)?;
    let end = start.checked_add(86_400)?.checked_sub(1)?;
    Some((start, end))
}

#[cfg(test)]
fn time_from_date(input: &str) -> Option<SystemTime> {
    date_bounds(input).map(|(start, _)| UNIX_EPOCH + std::time::Duration::from_secs(start))
}

fn parse_date(input: &str) -> Option<(i32, u32, u32)> {
    let mut parts = input.trim().split('-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) {
        return None;
    }
    let max_day = days_in_month(year, month)?;
    (1..=max_day).contains(&day).then_some((year, month, day))
}

fn days_in_month(year: i32, month: u32) -> Option<u32> {
    Some(match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => return None,
    })
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_from_civil(year: i32, month: u32, day: u32) -> Option<i64> {
    let (year, month) = if month <= 2 {
        (year.checked_sub(1)?, month + 12)
    } else {
        (year, month)
    };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month as i32 - 3) + 2) / 5 + day as i32 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era as i64 * 146_097 + day_of_era as i64 - 719_468)
}

fn scan_query(input: &str) -> Vec<QueryToken> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                if quoted {
                    if !current.is_empty() {
                        tokens.push(QueryToken {
                            value: std::mem::take(&mut current),
                            quoted: true,
                            kind: QueryTokenKind::Value,
                        });
                    }
                    quoted = false;
                } else {
                    if current == "-" {
                        quoted = true;
                        continue;
                    }
                    if !current.trim().is_empty() {
                        tokens.push(QueryToken {
                            value: std::mem::take(&mut current),
                            quoted: false,
                            kind: QueryTokenKind::Value,
                        });
                    }
                    quoted = true;
                }
            }
            '(' if !quoted => {
                if !current.is_empty() {
                    tokens.push(QueryToken {
                        value: std::mem::take(&mut current),
                        quoted: false,
                        kind: QueryTokenKind::Value,
                    });
                }
                tokens.push(QueryToken {
                    value: "(".to_string(),
                    quoted: false,
                    kind: QueryTokenKind::Open,
                });
            }
            ')' if !quoted => {
                if !current.is_empty() {
                    tokens.push(QueryToken {
                        value: std::mem::take(&mut current),
                        quoted: false,
                        kind: QueryTokenKind::Value,
                    });
                }
                tokens.push(QueryToken {
                    value: ")".to_string(),
                    quoted: false,
                    kind: QueryTokenKind::Close,
                });
            }
            '\\' if quoted => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            ch if ch.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    tokens.push(QueryToken {
                        value: std::mem::take(&mut current),
                        quoted: false,
                        kind: QueryTokenKind::Value,
                    });
                }
            }
            other => current.push(other),
        }
    }
    if !current.is_empty() {
        tokens.push(QueryToken {
            value: current,
            quoted,
            kind: QueryTokenKind::Value,
        });
    }
    tokens
}

#[cfg(test)]
mod tests;
