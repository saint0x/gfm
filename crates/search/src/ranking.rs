use crate::QueryFilter;
use gfm_types::{FileKind, FileRecord, MatchReason};

pub(crate) const EXACT_NAME: i64 = 1_000;
pub(crate) const PREFIX_NAME: i64 = 700;
pub(crate) const SUBSTRING_NAME: i64 = 300;
pub(crate) const NAME_TOKEN: i64 = 500;
pub(crate) const EXTENSION: i64 = 350;
pub(crate) const TAG: i64 = 325;
pub(crate) const PATH_COMPONENT: i64 = 250;
pub(crate) const CONTENT: i64 = 150;
pub(crate) const FUZZY_NAME: i64 = 100;
pub(crate) const PHRASE: i64 = 450;
pub(crate) const PROXIMITY: i64 = 375;
pub(crate) const USER_PINNED: i64 = 650;
pub(crate) const KIND_MATCH: i64 = 90;
pub(crate) const NAME_FREQUENCY: i64 = 12;
pub(crate) const PATH_FREQUENCY: i64 = 6;
pub(crate) const CONTENT_FREQUENCY: i64 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RankAccumulator {
    score: i64,
    reason: MatchReason,
    reason_weight: i64,
}

impl RankAccumulator {
    pub(crate) fn new(score: i64, reason: MatchReason) -> Self {
        Self {
            score,
            reason,
            reason_weight: score,
        }
    }

    pub(crate) fn add(&mut self, score: i64, reason: MatchReason) {
        self.score += score;
        if score > self.reason_weight {
            self.reason = reason;
            self.reason_weight = score;
        }
    }

    pub(crate) fn boost(&mut self, score: i64) {
        self.score += score;
    }

    pub(crate) fn finish(self) -> (i64, MatchReason) {
        (self.score, self.reason)
    }
}

pub(crate) fn capped_frequency(count: usize, points: i64) -> i64 {
    count.min(8) as i64 * points
}

pub(crate) fn kind_score(kind: FileKind) -> i64 {
    match kind {
        FileKind::Directory => KIND_MATCH,
        FileKind::File => KIND_MATCH,
        FileKind::Symlink => KIND_MATCH - 15,
        FileKind::Other => KIND_MATCH - 30,
    }
}

pub(crate) fn recency_score(record: &FileRecord) -> i64 {
    record
        .modified
        .and_then(|time| time.elapsed().ok())
        .map(|age| {
            let days = age.as_secs() / 86_400;
            100i64.saturating_sub(days.min(100) as i64)
        })
        .unwrap_or(0)
}

pub(crate) fn count_term(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    haystack.match_indices(needle).count()
}

pub(crate) fn filter_kind_matches(filter: &QueryFilter, kind: FileKind) -> bool {
    matches!(filter, QueryFilter::Kind(query_kind, false) if query_kind.matches_kind(kind))
}
