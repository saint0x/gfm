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
    let mut rarest = None;
    for term in terms {
        let ids = postings.get(term)?;
        if rarest.is_none_or(|current: &BTreeSet<FileId>| ids.len() < current.len()) {
            rarest = Some(ids);
        }
    }
    rarest
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_types::VolumeId;

    #[test]
    fn rarest_term_postings_selects_anchor_without_requiring_allocation() {
        let mut postings = BTreeMap::new();
        postings.insert(
            "common".to_string(),
            BTreeSet::from([
                FileId::new(VolumeId(1), 1),
                FileId::new(VolumeId(1), 2),
                FileId::new(VolumeId(1), 3),
            ]),
        );
        postings.insert(
            "rare".to_string(),
            BTreeSet::from([FileId::new(VolumeId(1), 2)]),
        );

        let terms = vec!["common".to_string(), "rare".to_string()];
        let rarest = rarest_term_postings(&terms, &postings).unwrap();

        assert_eq!(rarest, postings.get("rare").unwrap());
    }

    #[test]
    fn rarest_term_postings_fails_closed_when_any_term_is_missing() {
        let mut postings = BTreeMap::new();
        postings.insert(
            "present".to_string(),
            BTreeSet::from([FileId::new(VolumeId(1), 1)]),
        );

        let terms = vec!["present".to_string(), "missing".to_string()];

        assert!(rarest_term_postings(&terms, &postings).is_none());
    }
}
