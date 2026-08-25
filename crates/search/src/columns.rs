use crate::query::{normalize, tokenize};
use crate::{is_fuzzy_term, QueryFilter};
use gfm_types::{FileId, FileRecord};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RecordColumns {
    pub(super) name: String,
    pub(super) path: String,
    pub(super) extension: Option<String>,
    pub(super) tags: Vec<String>,
    comment: Option<String>,
    pub(super) name_tokens: Vec<String>,
    pub(super) path_tokens: Vec<String>,
    pub(super) metadata_tokens: Vec<String>,
    pub(super) fuzzy_terms: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRecordColumns {
    pub id: FileId,
    pub name: String,
    pub path: String,
    pub extension: Option<String>,
    pub tags: Vec<String>,
    pub comment: Option<String>,
}

impl RecordColumns {
    pub(super) fn from_record(record: &FileRecord) -> Self {
        let name = normalize(&record.name);
        let path = normalize(&record.path.to_string_lossy());
        let extension = record.extension().map(normalize);
        let mut tags = record
            .tags
            .iter()
            .map(|tag| normalize(tag))
            .filter(|tag| !tag.is_empty())
            .collect::<Vec<_>>();
        tags.sort();
        tags.dedup();
        let comment = record.finder_comment.as_deref().map(normalize);
        let name_tokens = tokenize(&name);
        let path_tokens = record
            .path
            .components()
            .filter_map(|component| component.as_os_str().to_str())
            .flat_map(|component| tokenize(&normalize(component)))
            .collect();
        let metadata_tokens = comment.as_deref().map(tokenize).unwrap_or_default();
        let fuzzy_terms = name_tokens
            .iter()
            .filter(|term| is_fuzzy_term(term))
            .cloned()
            .collect();
        Self {
            name,
            path,
            extension,
            tags,
            comment,
            name_tokens,
            path_tokens,
            metadata_tokens,
            fuzzy_terms,
        }
    }

    pub(super) fn from_search_columns(columns: &SearchRecordColumns) -> Self {
        let name = normalize(&columns.name);
        let path = normalize(&columns.path);
        let extension = columns.extension.as_deref().map(normalize);
        let mut tags = columns
            .tags
            .iter()
            .map(|tag| normalize(tag))
            .filter(|tag| !tag.is_empty())
            .collect::<Vec<_>>();
        tags.sort();
        tags.dedup();
        let comment = columns.comment.as_deref().map(normalize);
        let name_tokens = tokenize(&name);
        let path_tokens = tokenize(&path);
        let metadata_tokens = comment.as_deref().map(tokenize).unwrap_or_default();
        let fuzzy_terms = name_tokens
            .iter()
            .filter(|term| is_fuzzy_term(term))
            .cloned()
            .collect();
        Self {
            name,
            path,
            extension,
            tags,
            comment,
            name_tokens,
            path_tokens,
            metadata_tokens,
            fuzzy_terms,
        }
    }

    pub(super) fn contains_term(&self, term: &str) -> bool {
        self.name.contains(term)
            || self.path.contains(term)
            || self.tags.iter().any(|tag| tag.contains(term))
            || self
                .comment
                .as_deref()
                .is_some_and(|comment| comment.contains(term))
    }

    pub(super) fn matches_phrase(&self, phrase: &str) -> bool {
        self.name.contains(phrase)
            || self.path.contains(phrase)
            || self
                .comment
                .as_deref()
                .is_some_and(|comment| comment.contains(phrase))
    }
}

pub(super) fn filter_matches_columns(
    filter: &QueryFilter,
    record: &FileRecord,
    columns: &RecordColumns,
) -> bool {
    let positive = match filter {
        QueryFilter::Name(value, _) => columns.name.contains(value),
        QueryFilter::Path(value, _) => columns.path.contains(value),
        QueryFilter::Extension(value, _) => columns
            .extension
            .as_deref()
            .is_some_and(|extension| extension == value),
        QueryFilter::Tag(value, _) => columns.tags.iter().any(|tag| tag == value),
        QueryFilter::Scope(_, _) => return filter.matches(record),
        QueryFilter::Kind(kind, _) => kind.matches_kind(record.kind),
        QueryFilter::Size(_, _) | QueryFilter::Date(_, _, _) => return filter.matches(record),
    };
    if filter_is_negative(filter) {
        !positive
    } else {
        positive
    }
}

fn filter_is_negative(filter: &QueryFilter) -> bool {
    match filter {
        QueryFilter::Name(_, negative)
        | QueryFilter::Path(_, negative)
        | QueryFilter::Extension(_, negative)
        | QueryFilter::Tag(_, negative)
        | QueryFilter::Scope(_, negative)
        | QueryFilter::Kind(_, negative)
        | QueryFilter::Size(_, negative)
        | QueryFilter::Date(_, _, negative) => *negative,
    }
}
