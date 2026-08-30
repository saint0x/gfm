use crate::{ContentQueryLoadReport, LiveIndex};
use gfm_jobs::Cancellation;
use gfm_search::{SearchLookupBudget, SearchLookupTelemetry, SearchQuery, SearchQueryReport};
use gfm_store::{MmapContentSet, MmapRecordArchive};
use gfm_types::{ContentPosting, FileId, FileRecord, Result};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

const CONTENT_RECORD_CACHE_CAPACITY: usize = 8192;
const CONTENT_POSTING_CACHE_CAPACITY: usize = 512;
const CONTENT_QUERY_RESULT_CACHE_CAPACITY: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentQuerySessionReport {
    pub load: ContentQueryLoadReport,
    pub search: SearchQueryReport,
    pub posting_cache_hits: usize,
    pub posting_cache_misses: usize,
    pub record_cache_hits: usize,
    pub record_cache_misses: usize,
    pub result_cache_hits: usize,
    pub result_cache_misses: usize,
}

#[derive(Debug)]
pub struct ContentIndexQuerySession {
    records: MmapRecordArchive,
    content: MmapContentSet,
    posting_cache: Mutex<ContentPostingCache>,
    posting_cache_hits: AtomicUsize,
    posting_cache_misses: AtomicUsize,
    record_cache: Mutex<ContentRecordCache>,
    record_cache_hits: AtomicUsize,
    record_cache_misses: AtomicUsize,
    result_cache: Mutex<ContentResultCache>,
    result_cache_hits: AtomicUsize,
    result_cache_misses: AtomicUsize,
}

impl ContentIndexQuerySession {
    pub fn open_content(
        records_path: impl AsRef<Path>,
        content_path: impl AsRef<Path>,
    ) -> Result<Self> {
        Self::open_content_cancellable(records_path, content_path, &Cancellation::default())
    }

    pub fn open_content_cancellable(
        records_path: impl AsRef<Path>,
        content_path: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<Self> {
        Self::open_set_cancellable(records_path, std::iter::once(content_path), cancellation)
    }

    pub fn open_set<I, P>(records_path: impl AsRef<Path>, content_paths: I) -> Result<Self>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        Self::open_set_cancellable(records_path, content_paths, &Cancellation::default())
    }

    pub fn open_set_cancellable<I, P>(
        records_path: impl AsRef<Path>,
        content_paths: I,
        cancellation: &Cancellation,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        cancellation.check()?;
        let records = MmapRecordArchive::open_checked(records_path, || cancellation.check())?;
        cancellation.check()?;
        let content = MmapContentSet::open_checked(content_paths, || cancellation.check())?;
        cancellation.check()?;
        Ok(Self {
            records,
            content,
            posting_cache: Mutex::new(ContentPostingCache::new(CONTENT_POSTING_CACHE_CAPACITY)),
            posting_cache_hits: AtomicUsize::new(0),
            posting_cache_misses: AtomicUsize::new(0),
            record_cache: Mutex::new(ContentRecordCache::new(CONTENT_RECORD_CACHE_CAPACITY)),
            record_cache_hits: AtomicUsize::new(0),
            record_cache_misses: AtomicUsize::new(0),
            result_cache: Mutex::new(ContentResultCache::new(CONTENT_QUERY_RESULT_CACHE_CAPACITY)),
            result_cache_hits: AtomicUsize::new(0),
            result_cache_misses: AtomicUsize::new(0),
        })
    }

    pub fn open_manifest(
        records_path: impl AsRef<Path>,
        manifest_path: impl AsRef<Path>,
    ) -> Result<Self> {
        Self::open_manifest_cancellable(records_path, manifest_path, &Cancellation::default())
    }

    pub fn open_manifest_cancellable(
        records_path: impl AsRef<Path>,
        manifest_path: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<Self> {
        cancellation.check()?;
        let records = MmapRecordArchive::open_checked(records_path, || cancellation.check())?;
        cancellation.check()?;
        let content =
            MmapContentSet::open_manifest_checked(manifest_path, || cancellation.check())?;
        cancellation.check()?;
        Ok(Self {
            records,
            content,
            posting_cache: Mutex::new(ContentPostingCache::new(CONTENT_POSTING_CACHE_CAPACITY)),
            posting_cache_hits: AtomicUsize::new(0),
            posting_cache_misses: AtomicUsize::new(0),
            record_cache: Mutex::new(ContentRecordCache::new(CONTENT_RECORD_CACHE_CAPACITY)),
            record_cache_hits: AtomicUsize::new(0),
            record_cache_misses: AtomicUsize::new(0),
            result_cache: Mutex::new(ContentResultCache::new(CONTENT_QUERY_RESULT_CACHE_CAPACITY)),
            result_cache_hits: AtomicUsize::new(0),
            result_cache_misses: AtomicUsize::new(0),
        })
    }

    pub fn indexed_records(&self) -> usize {
        self.records.len()
    }

    pub fn archive_count(&self) -> usize {
        self.content.archive_count()
    }

    pub fn posting_cache_telemetry(&self) -> (usize, usize) {
        (
            self.posting_cache_hits.load(Ordering::Relaxed),
            self.posting_cache_misses.load(Ordering::Relaxed),
        )
    }

    pub fn record_cache_telemetry(&self) -> (usize, usize) {
        (
            self.record_cache_hits.load(Ordering::Relaxed),
            self.record_cache_misses.load(Ordering::Relaxed),
        )
    }

    pub fn result_cache_telemetry(&self) -> (usize, usize) {
        (
            self.result_cache_hits.load(Ordering::Relaxed),
            self.result_cache_misses.load(Ordering::Relaxed),
        )
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<ContentQuerySessionReport> {
        self.search_with_budget(query, limit, SearchLookupBudget::default())
    }

    pub fn search_with_budget(
        &self,
        query: &str,
        limit: usize,
        budget: SearchLookupBudget,
    ) -> Result<ContentQuerySessionReport> {
        self.search_structured_with_budget_cancellable(
            &SearchQuery::parse(query),
            limit,
            budget,
            &Cancellation::default(),
        )
    }

    pub fn search_cancellable(
        &self,
        query: &str,
        limit: usize,
        cancellation: &Cancellation,
    ) -> Result<ContentQuerySessionReport> {
        self.search_with_budget_cancellable(
            query,
            limit,
            SearchLookupBudget::default(),
            cancellation,
        )
    }

    pub fn search_with_budget_cancellable(
        &self,
        query: &str,
        limit: usize,
        budget: SearchLookupBudget,
        cancellation: &Cancellation,
    ) -> Result<ContentQuerySessionReport> {
        self.search_structured_with_budget_cancellable(
            &SearchQuery::parse(query),
            limit,
            budget,
            cancellation,
        )
    }

    pub fn search_structured_with_budget_cancellable(
        &self,
        parsed: &SearchQuery,
        limit: usize,
        budget: SearchLookupBudget,
        cancellation: &Cancellation,
    ) -> Result<ContentQuerySessionReport> {
        cancellation.check()?;
        if parsed.is_empty() || limit == 0 {
            return Ok(empty_content_query_session_report());
        }
        let result_cache_key = content_query_result_cache_key(parsed, limit, budget);
        if let Some(mut report) = self.result_cache_lock().get(&result_cache_key) {
            self.result_cache_hits.fetch_add(1, Ordering::Relaxed);
            report.search.lookup = SearchLookupTelemetry::default();
            report.posting_cache_hits = 0;
            report.posting_cache_misses = 0;
            report.record_cache_hits = 0;
            report.record_cache_misses = 0;
            report.result_cache_hits = 1;
            report.result_cache_misses = 0;
            return Ok(report);
        }
        self.result_cache_misses.fetch_add(1, Ordering::Relaxed);
        let posting_hits_before = self.posting_cache_hits.load(Ordering::Relaxed);
        let posting_misses_before = self.posting_cache_misses.load(Ordering::Relaxed);
        let content_terms = parsed.content_candidate_terms();
        let has_content_terms = !content_terms.is_empty();
        let postings = self.postings_for_terms(content_terms, budget, cancellation)?;
        cancellation.check()?;
        let cache_hits_before = self.record_cache_hits.load(Ordering::Relaxed);
        let cache_misses_before = self.record_cache_misses.load(Ordering::Relaxed);
        let (live, load) = self.live_from_postings(postings, has_content_terms, cancellation)?;
        let hits = live.search_structured_with_volume_scope_cancellable(
            parsed,
            limit,
            &gfm_search::SearchVolumeScope::All,
            cancellation,
        )?;
        let report = ContentQuerySessionReport {
            load,
            search: SearchQueryReport {
                hits,
                lookup: SearchLookupTelemetry::default(),
            },
            posting_cache_hits: self
                .posting_cache_hits
                .load(Ordering::Relaxed)
                .saturating_sub(posting_hits_before),
            posting_cache_misses: self
                .posting_cache_misses
                .load(Ordering::Relaxed)
                .saturating_sub(posting_misses_before),
            record_cache_hits: self
                .record_cache_hits
                .load(Ordering::Relaxed)
                .saturating_sub(cache_hits_before),
            record_cache_misses: self
                .record_cache_misses
                .load(Ordering::Relaxed)
                .saturating_sub(cache_misses_before),
            result_cache_hits: 0,
            result_cache_misses: 1,
        };
        self.result_cache_lock()
            .insert(result_cache_key, report.clone());
        Ok(report)
    }

    fn postings_for_terms(
        &self,
        terms: Vec<String>,
        budget: SearchLookupBudget,
        cancellation: &Cancellation,
    ) -> Result<Vec<ContentPosting>> {
        let mut selected = BTreeSet::new();
        for term in terms {
            cancellation.check()?;
            let term = term.trim().to_lowercase();
            if !term.is_empty() {
                selected.insert(term);
            }
        }
        if selected.is_empty() {
            return Ok(Vec::new());
        }

        let mut postings = Vec::with_capacity(selected.len());
        let mut misses = Vec::new();
        {
            let cache = self.posting_cache_lock();
            for term in &selected {
                cancellation.check()?;
                let key = posting_cache_key(term, budget.max_content_ids_per_term);
                if let Some(cached) = cache.get(&key) {
                    self.posting_cache_hits.fetch_add(1, Ordering::Relaxed);
                    if let Some(posting) = cached {
                        postings.push(posting);
                    }
                } else {
                    self.posting_cache_misses.fetch_add(1, Ordering::Relaxed);
                    misses.push(term.clone());
                }
            }
        }

        for term in misses {
            cancellation.check()?;
            let (posting, truncated) = self
                .content
                .posting_for_term_limit(&term, budget.max_content_ids_per_term)?;
            if !truncated {
                self.posting_cache_lock().insert(
                    posting_cache_key(&term, budget.max_content_ids_per_term),
                    posting.clone(),
                );
            }
            if let Some(posting) = posting {
                postings.push(posting);
            }
        }

        postings.sort_by(|left, right| left.term.cmp(&right.term));
        Ok(postings)
    }

    fn posting_cache_lock(&self) -> MutexGuard<'_, ContentPostingCache> {
        self.posting_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn live_from_postings(
        &self,
        postings: Vec<ContentPosting>,
        has_content_terms: bool,
        cancellation: &Cancellation,
    ) -> Result<(LiveIndex, ContentQueryLoadReport)> {
        cancellation.check()?;
        let candidate_ids = content_candidate_ids_cancellable(&postings, cancellation)?;
        let has_content_postings = !postings.is_empty();
        let full_hydration =
            !has_content_terms || (has_content_postings && candidate_ids.is_empty());
        let candidate_count = candidate_ids.len();
        let (records, missing) = if full_hydration {
            self.hydrate_all_records(cancellation)?
        } else {
            self.hydrate_record_ids(candidate_ids, cancellation)?
        };
        cancellation.check()?;

        let content_keys = postings.len();
        let (live, _, _, _, _, _, _) = LiveIndex::from_records_with_sidecars(
            records,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            postings,
        );
        let records_loaded = live.indexed_records();
        Ok((
            live,
            ContentQueryLoadReport {
                content_keys,
                candidate_ids: candidate_count,
                records_loaded,
                records_missing: missing,
                full_hydration,
            },
        ))
    }

    fn hydrate_all_records(&self, cancellation: &Cancellation) -> Result<(Vec<FileRecord>, usize)> {
        let mut records = Vec::with_capacity(self.records.len());
        for index in 0..self.records.len() {
            cancellation.check()?;
            records.push(self.records.record(index)?);
        }
        Ok((records, 0))
    }

    fn hydrate_record_ids(
        &self,
        ids: BTreeSet<FileId>,
        cancellation: &Cancellation,
    ) -> Result<(Vec<FileRecord>, usize)> {
        if ids.is_empty() {
            return Ok((Vec::new(), 0));
        }

        let mut records_by_id = HashMap::new();
        let mut misses = Vec::new();
        {
            let cache = self.record_cache_lock();
            for id in &ids {
                cancellation.check()?;
                if let Some(record) = cache.get(*id) {
                    self.record_cache_hits.fetch_add(1, Ordering::Relaxed);
                    records_by_id.insert(*id, record);
                } else {
                    self.record_cache_misses.fetch_add(1, Ordering::Relaxed);
                    misses.push(*id);
                }
            }
        }

        cancellation.check()?;
        let batch = self
            .records
            .records_for_sorted_ids(misses.iter().copied())?;
        let missing = batch.missing;
        {
            let mut cache = self.record_cache_lock();
            for record in batch.records {
                cancellation.check()?;
                cache.insert(record.id, record.clone());
                records_by_id.insert(record.id, record);
            }
        }

        let records = ids
            .into_iter()
            .filter_map(|id| records_by_id.remove(&id))
            .collect::<Vec<_>>();
        Ok((records, missing))
    }

    fn record_cache_lock(&self) -> MutexGuard<'_, ContentRecordCache> {
        self.record_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn result_cache_lock(&self) -> MutexGuard<'_, ContentResultCache> {
        self.result_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn content_candidate_ids_cancellable(
    postings: &[ContentPosting],
    cancellation: &Cancellation,
) -> Result<BTreeSet<FileId>> {
    let mut ids = BTreeSet::new();
    for posting in postings {
        cancellation.check()?;
        for id in &posting.ids {
            cancellation.check()?;
            ids.insert(*id);
        }
        for positions in &posting.positions {
            cancellation.check()?;
            ids.insert(positions.id);
        }
    }
    Ok(ids)
}

fn empty_content_query_session_report() -> ContentQuerySessionReport {
    ContentQuerySessionReport {
        load: ContentQueryLoadReport::default(),
        search: SearchQueryReport {
            hits: Vec::new(),
            lookup: SearchLookupTelemetry::default(),
        },
        posting_cache_hits: 0,
        posting_cache_misses: 0,
        record_cache_hits: 0,
        record_cache_misses: 0,
        result_cache_hits: 0,
        result_cache_misses: 0,
    }
}

fn posting_cache_key(term: &str, limit: usize) -> String {
    format!("{limit}:{term}")
}

fn content_query_result_cache_key(
    query: &SearchQuery,
    limit: usize,
    budget: SearchLookupBudget,
) -> String {
    format!(
        "{}\0{}\0{}",
        query.canonical_cache_key(),
        limit,
        budget.max_content_ids_per_term
    )
}

#[derive(Debug)]
struct ContentPostingCache {
    capacity: usize,
    order: VecDeque<String>,
    values: HashMap<String, Option<ContentPosting>>,
}

impl ContentPostingCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            order: VecDeque::with_capacity(capacity),
            values: HashMap::new(),
        }
    }

    fn get(&self, key: &str) -> Option<Option<ContentPosting>> {
        self.values.get(key).cloned()
    }

    fn insert(&mut self, key: String, posting: Option<ContentPosting>) {
        if self.capacity == 0 {
            return;
        }
        if !self.values.contains_key(&key) {
            self.order.push_back(key.clone());
        }
        self.values.insert(key, posting);
        while self.values.len() > self.capacity {
            let Some(expired) = self.order.pop_front() else {
                break;
            };
            self.values.remove(&expired);
        }
    }
}

#[derive(Debug)]
struct ContentRecordCache {
    capacity: usize,
    order: VecDeque<FileId>,
    values: HashMap<FileId, FileRecord>,
}

#[derive(Debug)]
struct ContentResultCache {
    capacity: usize,
    order: VecDeque<String>,
    values: HashMap<String, ContentQuerySessionReport>,
}

impl ContentResultCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            order: VecDeque::with_capacity(capacity),
            values: HashMap::new(),
        }
    }

    fn get(&self, key: &str) -> Option<ContentQuerySessionReport> {
        self.values.get(key).cloned()
    }

    fn insert(&mut self, key: String, report: ContentQuerySessionReport) {
        if self.capacity == 0 {
            return;
        }
        if !self.values.contains_key(&key) {
            self.order.push_back(key.clone());
        }
        self.values.insert(key, report);
        while self.values.len() > self.capacity {
            let Some(expired) = self.order.pop_front() else {
                break;
            };
            self.values.remove(&expired);
        }
    }
}

impl ContentRecordCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            order: VecDeque::with_capacity(capacity),
            values: HashMap::new(),
        }
    }

    fn get(&self, id: FileId) -> Option<FileRecord> {
        self.values.get(&id).cloned()
    }

    fn insert(&mut self, id: FileId, record: FileRecord) {
        if self.capacity == 0 {
            return;
        }
        if !self.values.contains_key(&id) {
            self.order.push_back(id);
        }
        self.values.insert(id, record);
        while self.values.len() > self.capacity {
            let Some(expired) = self.order.pop_front() else {
                break;
            };
            self.values.remove(&expired);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_jobs::Cancellation;
    use gfm_store::{write_content_postings, write_records};
    use gfm_types::{ContentPositions, FileKind, GfmError, VolumeId};
    use std::fs;
    use std::panic::{self, AssertUnwindSafe};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn content_session_recovers_poisoned_posting_cache() {
        let fixture = ContentSessionFixture::new("posting-cache");
        let session = fixture.session();

        poison_posting_cache(&session);
        let report = session.search("needle", 5).unwrap();

        assert_eq!(report.search.hits.len(), 1);
        assert_eq!(report.search.hits[0].record.name, "Needle.md");
        assert_eq!(report.posting_cache_misses, 1);
        assert_eq!(report.record_cache_misses, 1);
    }

    #[test]
    fn content_session_recovers_poisoned_record_cache() {
        let fixture = ContentSessionFixture::new("record-cache");
        let session = fixture.session();
        let first = session.search("needle", 5).unwrap();
        assert_eq!(first.search.hits.len(), 1);

        poison_record_cache(&session);
        let second = session.search("needle", 6).unwrap();

        assert_eq!(second.search.hits.len(), 1);
        assert_eq!(second.search.hits[0].record.name, "Needle.md");
        assert_eq!(second.posting_cache_hits, 1);
        assert_eq!(second.record_cache_hits, 1);
        assert_eq!(second.result_cache_hits, 0);
        assert_eq!(second.result_cache_misses, 1);
    }

    #[test]
    fn content_session_reuses_normalized_query_results() {
        let fixture = ContentSessionFixture::new("result-cache");
        let session = fixture.session();

        let first = session.search("  Needle  ", 5).unwrap();
        let second = session.search("needle", 5).unwrap();

        assert_eq!(first.search.hits, second.search.hits);
        assert_eq!(first.posting_cache_hits, 0);
        assert_eq!(first.posting_cache_misses, 1);
        assert_eq!(first.record_cache_hits, 0);
        assert_eq!(first.record_cache_misses, 1);
        assert_eq!(first.result_cache_hits, 0);
        assert_eq!(first.result_cache_misses, 1);
        assert_eq!(second.posting_cache_hits, 0);
        assert_eq!(second.posting_cache_misses, 0);
        assert_eq!(second.record_cache_hits, 0);
        assert_eq!(second.record_cache_misses, 0);
        assert_eq!(second.result_cache_hits, 1);
        assert_eq!(second.result_cache_misses, 0);
        assert_eq!(session.result_cache_telemetry(), (1, 1));
    }

    #[test]
    fn content_session_empty_and_zero_limit_queries_skip_cache_work() {
        let fixture = ContentSessionFixture::new("empty-query");
        let session = fixture.session();

        let empty = session.search("   ", 5).unwrap();
        let zero_limit = session.search("needle", 0).unwrap();

        assert!(empty.search.hits.is_empty());
        assert_eq!(empty.load, ContentQueryLoadReport::default());
        assert_eq!(zero_limit, empty);
        assert_eq!(session.posting_cache_telemetry(), (0, 0));
        assert_eq!(session.record_cache_telemetry(), (0, 0));
        assert_eq!(session.result_cache_telemetry(), (0, 0));
    }

    #[test]
    fn content_session_honors_pre_cancelled_queries_without_cache_work() {
        let fixture = ContentSessionFixture::new("pre-cancelled");
        let session = fixture.session();
        let cancellation = Cancellation::default();
        cancellation.cancel();

        let result = session.search_cancellable("needle", 5, &cancellation);

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert_eq!(session.posting_cache_telemetry(), (0, 0));
        assert_eq!(session.record_cache_telemetry(), (0, 0));
        assert_eq!(session.result_cache_telemetry(), (0, 0));
    }

    #[test]
    fn content_candidate_expansion_honors_cancelled_tokens() {
        let cancellation = Cancellation::default();
        cancellation.cancel();
        let posting = ContentPosting {
            term: "needle".to_string(),
            ids: vec![FileId::new(VolumeId(1), 1)],
            positions: vec![ContentPositions {
                id: FileId::new(VolumeId(1), 2),
                positions: vec![0],
            }],
        };

        let result = content_candidate_ids_cancellable(&[posting], &cancellation);

        assert!(matches!(result, Err(GfmError::Cancelled)));
    }

    fn poison_posting_cache(session: &ContentIndexQuerySession) {
        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            let _guard = session
                .posting_cache
                .lock()
                .expect("initial content posting cache lock");
            panic!("poison content posting cache");
        }));
    }

    fn poison_record_cache(session: &ContentIndexQuerySession) {
        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            let _guard = session
                .record_cache
                .lock()
                .expect("initial content record cache lock");
            panic!("poison content record cache");
        }));
    }

    struct ContentSessionFixture {
        root: PathBuf,
        records: PathBuf,
        content: PathBuf,
    }

    impl ContentSessionFixture {
        fn new(name: &str) -> Self {
            let root = temp_dir(&format!("gfm-content-session-{name}"));
            let records = root.join("records.gfmidx");
            let content = root.join("content.gfmcontent");
            let id = FileId::new(VolumeId(1), 42);
            write_records(&records, &[record(id)]).unwrap();
            write_content_postings(
                &content,
                &[ContentPosting {
                    term: "needle".to_string(),
                    ids: vec![id],
                    positions: vec![ContentPositions {
                        id,
                        positions: vec![1],
                    }],
                }],
            )
            .unwrap();
            Self {
                root,
                records,
                content,
            }
        }

        fn session(&self) -> ContentIndexQuerySession {
            ContentIndexQuerySession::open_content(&self.records, &self.content).unwrap()
        }
    }

    impl Drop for ContentSessionFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn record(id: FileId) -> FileRecord {
        FileRecord {
            id,
            parent: None,
            path: PathBuf::from("/tmp/Needle.md"),
            name: "Needle.md".to_string(),
            kind: FileKind::File,
            len: 6,
            mode: 0o100644,
            owner: 501,
            group: 20,
            xattrs_digest: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: Vec::new(),
            finder_comment: None,
        }
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
