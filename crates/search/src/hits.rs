use gfm_types::SearchHit;
use std::cmp::Reverse;

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

#[derive(Debug)]
pub(crate) struct BoundedHitMerge {
    limit: usize,
    hits: Vec<SearchHit>,
}

impl BoundedHitMerge {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            limit,
            hits: Vec::with_capacity(limit),
        }
    }

    pub(crate) fn push(&mut self, hit: SearchHit) {
        if self.limit == 0 {
            return;
        }
        self.hits.push(hit);
        if self.hits.len() > self.limit.saturating_mul(2) {
            self.trim();
        }
    }

    pub(crate) fn extend(&mut self, hits: Vec<SearchHit>) {
        if self.limit == 0 || hits.is_empty() {
            return;
        }
        self.hits.extend(hits);
        if self.hits.len() > self.limit.saturating_mul(2) {
            self.trim();
        }
    }

    pub(crate) fn into_sorted_hits(mut self) -> Vec<SearchHit> {
        self.trim();
        self.hits
    }

    fn trim(&mut self) {
        sort_hits(&mut self.hits);
        self.hits.truncate(self.limit);
    }
}

pub(crate) fn top_hits(hits: Vec<SearchHit>, limit: usize) -> Vec<SearchHit> {
    let mut merge = BoundedHitMerge::new(limit);
    merge.extend(hits);
    merge.into_sorted_hits()
}
