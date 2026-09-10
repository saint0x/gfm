use gfm_types::SearchHit;
use std::cmp::Reverse;

#[cfg(test)]
pub(crate) fn sort_hits(hits: &mut [SearchHit]) {
    hits.sort_by_cached_key(|hit| {
        (
            Reverse(hit.score),
            hit.record.name.to_lowercase(),
            hit.record.path.to_string_lossy().into_owned(),
            hit.record.id,
        )
    });
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SearchHitSortKey {
    score: Reverse<i64>,
    name: String,
    path: String,
    id: gfm_types::FileId,
}

impl SearchHitSortKey {
    fn from_hit(hit: &SearchHit) -> Self {
        Self {
            score: Reverse(hit.score),
            name: hit.record.name.to_lowercase(),
            path: hit.record.path.to_string_lossy().into_owned(),
            id: hit.record.id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KeyedSearchHit {
    key: SearchHitSortKey,
    hit: SearchHit,
}

impl KeyedSearchHit {
    fn new(hit: SearchHit) -> Self {
        Self {
            key: SearchHitSortKey::from_hit(&hit),
            hit,
        }
    }
}

#[derive(Debug)]
pub(crate) struct BoundedHitMerge {
    limit: usize,
    hits: Vec<KeyedSearchHit>,
    #[cfg(test)]
    max_retained_len: usize,
}

impl BoundedHitMerge {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            limit,
            hits: Vec::with_capacity(limit),
            #[cfg(test)]
            max_retained_len: 0,
        }
    }

    pub(crate) fn push(&mut self, hit: SearchHit) {
        if self.limit == 0 {
            return;
        }
        self.hits.push(KeyedSearchHit::new(hit));
        self.record_retained_len();
        if self.hits.len() > self.limit.saturating_mul(2) {
            self.trim();
        }
    }

    pub(crate) fn extend(&mut self, hits: Vec<SearchHit>) {
        if self.limit == 0 || hits.is_empty() {
            return;
        }
        for hit in hits {
            self.push(hit);
        }
    }

    pub(crate) fn into_sorted_hits(mut self) -> Vec<SearchHit> {
        self.trim();
        self.hits.into_iter().map(|keyed| keyed.hit).collect()
    }

    fn trim(&mut self) {
        self.hits.sort_by(|left, right| left.key.cmp(&right.key));
        self.hits.truncate(self.limit);
        self.record_retained_len();
    }

    #[cfg(test)]
    pub(crate) fn max_retained_len(&self) -> usize {
        self.max_retained_len
    }

    #[cfg(test)]
    fn record_retained_len(&mut self) {
        self.max_retained_len = self.max_retained_len.max(self.hits.len());
    }

    #[cfg(not(test))]
    fn record_retained_len(&mut self) {}
}

pub(crate) fn top_hits(hits: Vec<SearchHit>, limit: usize) -> Vec<SearchHit> {
    let mut merge = BoundedHitMerge::new(limit);
    merge.extend(hits);
    merge.into_sorted_hits()
}
