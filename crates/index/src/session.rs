use crate::{content_query_terms, ContentQueryLoadReport, LiveIndex};
use gfm_search::{SearchLookupBudget, SearchLookupTelemetry, SearchQueryReport};
use gfm_store::{MmapContentSet, MmapRecordArchive};
use gfm_types::{ContentPosting, FileId, FileRecord, GfmError, Result};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

const CONTENT_RECORD_CACHE_CAPACITY: usize = 8192;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentQuerySessionReport {
    pub load: ContentQueryLoadReport,
    pub search: SearchQueryReport,
    pub record_cache_hits: usize,
    pub record_cache_misses: usize,
}

#[derive(Debug)]
pub struct ContentIndexQuerySession {
    records: MmapRecordArchive,
    content: MmapContentSet,
    record_cache: Mutex<ContentRecordCache>,
    record_cache_hits: AtomicUsize,
    record_cache_misses: AtomicUsize,
}

impl ContentIndexQuerySession {
    pub fn open_content(
        records_path: impl AsRef<Path>,
        content_path: impl AsRef<Path>,
    ) -> Result<Self> {
        Self::open_set(records_path, std::iter::once(content_path))
    }

    pub fn open_set<I, P>(records_path: impl AsRef<Path>, content_paths: I) -> Result<Self>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        Ok(Self {
            records: MmapRecordArchive::open(records_path)?,
            content: MmapContentSet::open(content_paths)?,
            record_cache: Mutex::new(ContentRecordCache::new(CONTENT_RECORD_CACHE_CAPACITY)),
            record_cache_hits: AtomicUsize::new(0),
            record_cache_misses: AtomicUsize::new(0),
        })
    }

    pub fn open_manifest(
        records_path: impl AsRef<Path>,
        manifest_path: impl AsRef<Path>,
    ) -> Result<Self> {
        Ok(Self {
            records: MmapRecordArchive::open(records_path)?,
            content: MmapContentSet::open_manifest(manifest_path)?,
            record_cache: Mutex::new(ContentRecordCache::new(CONTENT_RECORD_CACHE_CAPACITY)),
            record_cache_hits: AtomicUsize::new(0),
            record_cache_misses: AtomicUsize::new(0),
        })
    }

    pub fn indexed_records(&self) -> usize {
        self.records.len()
    }

    pub fn archive_count(&self) -> usize {
        self.content.archive_count()
    }

    pub fn record_cache_telemetry(&self) -> (usize, usize) {
        (
            self.record_cache_hits.load(Ordering::Relaxed),
            self.record_cache_misses.load(Ordering::Relaxed),
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
        let postings = self.content.postings_for_terms_limit(
            content_query_terms(query),
            budget.max_content_ids_per_term,
        )?;
        let cache_hits_before = self.record_cache_hits.load(Ordering::Relaxed);
        let cache_misses_before = self.record_cache_misses.load(Ordering::Relaxed);
        let (live, load) = self.live_from_postings(postings)?;
        let hits = live.search(query, limit);
        Ok(ContentQuerySessionReport {
            load,
            search: SearchQueryReport {
                hits,
                lookup: SearchLookupTelemetry::default(),
            },
            record_cache_hits: self
                .record_cache_hits
                .load(Ordering::Relaxed)
                .saturating_sub(cache_hits_before),
            record_cache_misses: self
                .record_cache_misses
                .load(Ordering::Relaxed)
                .saturating_sub(cache_misses_before),
        })
    }

    fn live_from_postings(
        &self,
        postings: Vec<ContentPosting>,
    ) -> Result<(LiveIndex, ContentQueryLoadReport)> {
        let candidate_ids = content_candidate_ids(&postings);
        let full_hydration = candidate_ids.is_empty();
        let candidate_count = candidate_ids.len();
        let (records, missing) = if full_hydration {
            self.hydrate_all_records()?
        } else {
            self.hydrate_record_ids(candidate_ids)?
        };

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

    fn hydrate_all_records(&self) -> Result<(Vec<FileRecord>, usize)> {
        let mut records = Vec::with_capacity(self.records.len());
        for index in 0..self.records.len() {
            records.push(self.records.record(index)?);
        }
        Ok((records, 0))
    }

    fn hydrate_record_ids(&self, ids: BTreeSet<FileId>) -> Result<(Vec<FileRecord>, usize)> {
        if ids.is_empty() {
            return Ok((Vec::new(), 0));
        }

        let mut records_by_id = HashMap::new();
        let mut misses = Vec::new();
        {
            let cache = self
                .record_cache
                .lock()
                .map_err(|_| GfmError::Format("content record cache lock poisoned".to_string()))?;
            for id in &ids {
                if let Some(record) = cache.get(*id) {
                    self.record_cache_hits.fetch_add(1, Ordering::Relaxed);
                    records_by_id.insert(*id, record);
                } else {
                    self.record_cache_misses.fetch_add(1, Ordering::Relaxed);
                    misses.push(*id);
                }
            }
        }

        let batch = self
            .records
            .records_for_sorted_ids(misses.iter().copied())?;
        let missing = batch.missing;
        {
            let mut cache = self
                .record_cache
                .lock()
                .map_err(|_| GfmError::Format("content record cache lock poisoned".to_string()))?;
            for record in batch.records {
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
}

fn content_candidate_ids(postings: &[ContentPosting]) -> BTreeSet<FileId> {
    let mut ids = BTreeSet::new();
    for posting in postings {
        ids.extend(posting.ids.iter().copied());
        ids.extend(posting.positions.iter().map(|positions| positions.id));
    }
    ids
}

#[derive(Debug)]
struct ContentRecordCache {
    capacity: usize,
    order: VecDeque<FileId>,
    values: HashMap<FileId, FileRecord>,
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
