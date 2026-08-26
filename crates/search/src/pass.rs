use gfm_types::FileId;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchPass {
    Hot,
    Full,
}

impl SearchPass {
    pub(crate) fn includes_deep(self) -> bool {
        self == Self::Full
    }
}

pub(crate) fn rarest_term_postings<'a>(
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
