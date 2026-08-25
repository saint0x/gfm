use gfm_content::Extractor;
use gfm_fs::{scan_tree, ScanOptions};
use gfm_jobs::Cancellation;
pub use gfm_search::{
    SearchFuzzyPosting, SearchLookup, SearchLookupBudget, SearchLookupTelemetry,
    SearchMetadataField, SearchMetadataPosting, SearchPrefixPosting, SearchQueryReport,
    SearchRecordColumns, SearchStreamStage,
};
use gfm_search::{SearchQuery, SearchStreamBatch, ShardedSearchIndex};
use gfm_store::{
    compact_content_segments, compact_content_segments_with_policy, plan_content_segment_merge,
    read_content_postings, read_records, summarize_content_segment, write_content_postings,
    write_content_segment, write_records, MmapContentSet, MmapFuzzyArchive, MmapMetadataArchive,
    MmapPrefixArchive, MmapRecordArchive, MmapRecordColumns,
};
pub use gfm_store::{
    ContentArchiveCleanupAction, ContentArchiveCleanupPlan, ContentArchiveCleanupPolicy,
    ContentArchiveCleanupReport, ContentArchiveManifest, ContentArchiveManifestEntry,
    ContentManifestPromotion, ContentMergeOutcome, ContentMergePolicy, ContentMergeTier,
};
use gfm_types::{
    ContentPosting, ContentSegment, DirectoryPage, FileEvent, FileEventKind, FileId, FileRecord,
    GfmError, Result, ScanIssue, SearchHit,
};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

mod backpressure;
mod cursor;
mod metadata;
mod progress;
mod recovery;
mod rename;
mod repair;
mod scan;
mod state;
mod volume;

pub use backpressure::{
    EventBackpressureQueue, EventBackpressureReport, EventBackpressureSnapshot, EventPriority,
};
pub use cursor::{
    FseventsCursor, FseventsCursorHealth, FseventsResumeAction, FseventsResumePlan,
    FSEVENTS_CURSOR_SCHEMA_VERSION,
};
pub use metadata::{diff_metadata, MetadataUpdateReport};
pub use progress::{ScanProgressCheckpoint, SCAN_PROGRESS_SCHEMA_VERSION};
pub use recovery::{
    persistent_index_action_name, persistent_index_reason_name, plan_persistent_index_recovery,
    PersistentIndexAction, PersistentIndexPlan, PersistentIndexReason, PersistentIndexRecovery,
};
pub use rename::{correlate_rename, RenameCorrelationReport};
pub use repair::{RepairPriority, RepairReason, RepairSchedule, SubtreeRepairJob};
pub use scan::{FairScanReport, FairScanScheduler, FairScanSummary, ScanLane};
pub use state::{IndexVolumeState, INDEX_STATE_SCHEMA_VERSION};
pub use volume::{
    parse_volume_indexing_policy, volume_indexing_policy_name, IndexMountState, IndexVolumeClass,
    IndexVolumeDescriptor, VolumeIndexAction, VolumeIndexDecision, VolumeIndexPlan,
    VolumeIndexPolicy, VolumeIndexThrottle, VolumeThrottleClass,
};

pub fn content_query_terms(query: &str) -> Vec<String> {
    SearchQuery::parse(query).content_candidate_terms()
}

pub fn comment_query_terms(query: &str) -> Vec<String> {
    SearchQuery::parse(query).comment_candidate_terms()
}

pub fn tag_query_terms(query: &str) -> Vec<String> {
    SearchQuery::parse(query).tag_candidate_terms()
}

pub fn prefix_query_terms(query: &str) -> Vec<String> {
    SearchQuery::parse(query).prefix_candidate_terms()
}

pub fn fuzzy_query_keys(query: &str) -> Vec<String> {
    SearchQuery::parse(query).fuzzy_candidate_keys()
}

#[derive(Debug, Clone)]
pub struct IndexSnapshot {
    pub root: PathBuf,
    pub records: Vec<FileRecord>,
    pub inaccessible: Vec<ScanIssue>,
}

impl IndexSnapshot {
    pub fn from_page(page: DirectoryPage) -> Self {
        Self {
            root: page.root,
            records: page.entries,
            inaccessible: page.inaccessible,
        }
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        let mut index = ShardedSearchIndex::new();
        for record in self.records.iter().cloned() {
            index.insert(record);
        }
        index.query(query, limit)
    }

    pub fn stream_search(&self, query: &str, limit: usize) -> Result<Vec<SearchStreamBatch>> {
        let mut index = ShardedSearchIndex::new();
        for record in self.records.iter().cloned() {
            index.insert(record);
        }
        index.stream(query, limit)
    }

    pub fn search_cancellable(
        &self,
        query: &str,
        limit: usize,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchHit>> {
        let mut index = ShardedSearchIndex::new();
        for record in self.records.iter().cloned() {
            cancellation.check()?;
            index.insert(record);
        }
        index.query_cancellable(query, limit, cancellation)
    }

    pub fn search_with_content(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let mut live = self.clone().into_live();
        live.index_content(&Extractor::default())?;
        Ok(live.search(query, limit))
    }

    pub fn search_with_content_snippets(
        &self,
        query: &str,
        limit: usize,
        extractor: &Extractor,
        context_bytes: usize,
    ) -> Result<Vec<SearchHit>> {
        let mut live = self.clone().into_live();
        live.index_content(extractor)?;
        live.search_with_snippets(query, limit, extractor, context_bytes)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        write_records(path, &self.records)
    }

    pub fn volume_state(
        &self,
        records_path: impl Into<PathBuf>,
        previous: Option<&IndexVolumeState>,
    ) -> Result<IndexVolumeState> {
        IndexVolumeState::from_page(
            &DirectoryPage {
                root: self.root.clone(),
                entries: self.records.clone(),
                inaccessible: self.inaccessible.clone(),
            },
            records_path,
            previous,
        )
    }

    pub fn save_with_content(
        &self,
        records_path: impl AsRef<Path>,
        content_path: impl AsRef<Path>,
        extractor: &Extractor,
    ) -> Result<usize> {
        self.save(records_path)?;
        let mut live = self.clone().into_live();
        let indexed = live.index_content(extractor)?;
        live.save_content_postings(content_path)?;
        Ok(indexed)
    }

    pub fn save_content_segment(
        &self,
        segment_path: impl AsRef<Path>,
        extractor: &Extractor,
        tombstones: Vec<FileId>,
    ) -> Result<usize> {
        let mut live = self.clone().into_live();
        let indexed = live.index_content(extractor)?;
        let segment = ContentSegment {
            tombstones,
            postings: live.content_postings(),
        };
        write_content_segment(segment_path, &segment)?;
        Ok(indexed)
    }

    pub fn into_live(self) -> LiveIndex {
        LiveIndex::from_records(self.records)
    }
}

#[derive(Debug, Clone, Default)]
pub struct LiveIndex {
    index: ShardedSearchIndex,
}

#[derive(Debug)]
pub struct SearchArchiveLookup {
    prefixes: MmapPrefixArchive,
    fuzzy: MmapFuzzyArchive,
    prefix_cache: Mutex<HashMap<String, Vec<FileId>>>,
    fuzzy_cache: Mutex<HashMap<String, Vec<String>>>,
    prefix_requests: AtomicUsize,
    prefix_hits: AtomicUsize,
    prefix_misses: AtomicUsize,
    fuzzy_requests: AtomicUsize,
    fuzzy_hits: AtomicUsize,
    fuzzy_misses: AtomicUsize,
}

impl SearchArchiveLookup {
    pub fn open(
        prefixes: impl AsRef<Path>,
        fuzzy: impl AsRef<Path>,
    ) -> Result<SearchArchiveLookup> {
        Ok(Self {
            prefixes: MmapPrefixArchive::open(prefixes)?,
            fuzzy: MmapFuzzyArchive::open(fuzzy)?,
            prefix_cache: Mutex::new(HashMap::new()),
            fuzzy_cache: Mutex::new(HashMap::new()),
            prefix_requests: AtomicUsize::new(0),
            prefix_hits: AtomicUsize::new(0),
            prefix_misses: AtomicUsize::new(0),
            fuzzy_requests: AtomicUsize::new(0),
            fuzzy_hits: AtomicUsize::new(0),
            fuzzy_misses: AtomicUsize::new(0),
        })
    }

    pub fn indexed_prefixes(&self) -> usize {
        self.prefixes.indexed_prefixes()
    }

    pub fn indexed_fuzzy_keys(&self) -> usize {
        self.fuzzy.indexed_keys()
    }
}

impl SearchLookup for SearchArchiveLookup {
    fn prefix_ids(&self, prefix: &str) -> Result<Vec<FileId>> {
        self.prefix_requests.fetch_add(1, Ordering::Relaxed);
        if let Some(ids) = self
            .prefix_cache
            .lock()
            .map_err(|_| GfmError::Format("prefix lookup cache lock poisoned".to_string()))?
            .get(prefix)
            .cloned()
        {
            self.prefix_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(ids);
        }

        self.prefix_misses.fetch_add(1, Ordering::Relaxed);
        let ids = self.prefixes.ids_for(prefix)?;
        self.prefix_cache
            .lock()
            .map_err(|_| GfmError::Format("prefix lookup cache lock poisoned".to_string()))?
            .insert(prefix.to_string(), ids.clone());
        Ok(ids)
    }

    fn fuzzy_terms(&self, key: &str) -> Result<Vec<String>> {
        self.fuzzy_requests.fetch_add(1, Ordering::Relaxed);
        if let Some(terms) = self
            .fuzzy_cache
            .lock()
            .map_err(|_| GfmError::Format("fuzzy lookup cache lock poisoned".to_string()))?
            .get(key)
            .cloned()
        {
            self.fuzzy_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(terms);
        }

        self.fuzzy_misses.fetch_add(1, Ordering::Relaxed);
        let terms = self.fuzzy.terms_for(key)?;
        self.fuzzy_cache
            .lock()
            .map_err(|_| GfmError::Format("fuzzy lookup cache lock poisoned".to_string()))?
            .insert(key.to_string(), terms.clone());
        Ok(terms)
    }

    fn cache_telemetry(&self) -> SearchLookupTelemetry {
        SearchLookupTelemetry {
            prefix_lookup_requests: self.prefix_requests.load(Ordering::Relaxed),
            prefix_cache_hits: self.prefix_hits.load(Ordering::Relaxed),
            prefix_cache_misses: self.prefix_misses.load(Ordering::Relaxed),
            fuzzy_lookup_requests: self.fuzzy_requests.load(Ordering::Relaxed),
            fuzzy_cache_hits: self.fuzzy_hits.load(Ordering::Relaxed),
            fuzzy_cache_misses: self.fuzzy_misses.load(Ordering::Relaxed),
            ..SearchLookupTelemetry::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexFootprintSpec {
    pub records: PathBuf,
    pub columns: Option<PathBuf>,
    pub metadata: Option<PathBuf>,
    pub prefixes: Option<PathBuf>,
    pub fuzzy: Option<PathBuf>,
    pub content_manifest: Option<PathBuf>,
    pub content_segments: Vec<PathBuf>,
    pub merge_policy: ContentMergePolicy,
    pub compaction_pressure: CompactionPressure,
    pub density_policy: IndexDensityPolicy,
}

impl IndexFootprintSpec {
    pub fn new(records: impl Into<PathBuf>) -> Self {
        Self {
            records: records.into(),
            columns: None,
            metadata: None,
            prefixes: None,
            fuzzy: None,
            content_manifest: None,
            content_segments: Vec::new(),
            merge_policy: ContentMergePolicy::default(),
            compaction_pressure: CompactionPressure::default(),
            density_policy: IndexDensityPolicy::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionPressure {
    pub io: IoPressure,
    pub thermal: ThermalState,
    pub battery: BatteryState,
    pub user_activity: UserActivity,
}

impl Default for CompactionPressure {
    fn default() -> Self {
        Self {
            io: IoPressure::Nominal,
            thermal: ThermalState::Nominal,
            battery: BatteryState::AcPower,
            user_activity: UserActivity::Idle,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoPressure {
    Nominal,
    Elevated,
    Saturated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermalState {
    Nominal,
    Fair,
    Serious,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryState {
    AcPower,
    Battery,
    LowPower,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserActivity {
    Idle,
    Active,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexDensityPolicy {
    pub target_bytes_per_record: u64,
}

impl Default for IndexDensityPolicy {
    fn default() -> Self {
        Self {
            target_bytes_per_record: 1 << 20,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexFootprintReport {
    pub record_count: usize,
    pub record_bytes: u64,
    pub column_count: usize,
    pub column_bytes: u64,
    pub column_string_pool_bytes: usize,
    pub metadata_terms: usize,
    pub metadata_bytes: u64,
    pub prefix_keys: usize,
    pub prefix_bytes: u64,
    pub fuzzy_keys: usize,
    pub fuzzy_bytes: u64,
    pub content_archives: usize,
    pub content_terms: usize,
    pub content_bytes: u64,
    pub segment_count: usize,
    pub segment_bytes: u64,
    pub segment_postings: usize,
    pub tombstone_segments: usize,
    pub tombstones: usize,
    pub total_bytes: u64,
    pub bytes_per_record: u64,
    pub compaction: IndexCompactionSchedule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexCompactionSchedule {
    pub scheduled: bool,
    pub tier: ContentMergeTier,
    pub merge_segments: Vec<PathBuf>,
    pub retained_segments: Vec<PathBuf>,
    pub merge_bytes: u64,
    pub effective_max_merge_bytes: u64,
    pub tombstone_segments: usize,
    pub reason: CompactionReason,
    pub action: CompactionAction,
    pub pressure: CompactionPressure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionReason {
    Tombstones,
    IndexDensity,
    TierPressure,
    BelowThreshold,
    NoSegments,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionAction {
    Run,
    Throttle,
    Defer,
}

pub fn inspect_index_footprint(spec: &IndexFootprintSpec) -> Result<IndexFootprintReport> {
    let records = MmapRecordArchive::open(&spec.records)?;
    let record_count = records.len();
    let record_bytes = mapped_bytes(&spec.records, records.mapped_len())?;

    let (column_count, column_bytes, column_string_pool_bytes) = if let Some(path) = &spec.columns {
        let archive = MmapRecordColumns::open(path)?;
        (
            archive.len(),
            mapped_bytes(path, archive.mapped_len())?,
            archive.string_pool_len(),
        )
    } else {
        (0, 0, 0)
    };

    let (metadata_terms, metadata_bytes) = if let Some(path) = &spec.metadata {
        let archive = MmapMetadataArchive::open(path)?;
        (
            archive.indexed_terms(),
            mapped_bytes(path, archive.mapped_len())?,
        )
    } else {
        (0, 0)
    };

    let (prefix_keys, prefix_bytes) = if let Some(path) = &spec.prefixes {
        let archive = MmapPrefixArchive::open(path)?;
        (
            archive.indexed_prefixes(),
            mapped_bytes(path, archive.mapped_len())?,
        )
    } else {
        (0, 0)
    };

    let (fuzzy_keys, fuzzy_bytes) = if let Some(path) = &spec.fuzzy {
        let archive = MmapFuzzyArchive::open(path)?;
        (
            archive.indexed_keys(),
            mapped_bytes(path, archive.mapped_len())?,
        )
    } else {
        (0, 0)
    };

    let (content_archives, content_terms, content_bytes) =
        if let Some(path) = &spec.content_manifest {
            let content = MmapContentSet::open_manifest(path)?;
            (
                content.archive_count(),
                content.indexed_terms(),
                content.mapped_len() as u64,
            )
        } else {
            (0, 0, 0)
        };

    let summaries = spec
        .content_segments
        .iter()
        .map(|path| summarize_content_segment(path, &spec.merge_policy))
        .collect::<Result<Vec<_>>>()?;
    let segment_count = summaries.len();
    let segment_bytes = summaries
        .iter()
        .fold(0u64, |total, summary| total.saturating_add(summary.bytes));
    let segment_postings = summaries.iter().map(|summary| summary.postings).sum();
    let tombstone_segments = summaries
        .iter()
        .filter(|summary| summary.tombstones > 0)
        .count();
    let tombstones = summaries.iter().map(|summary| summary.tombstones).sum();
    let total_bytes = [
        record_bytes,
        column_bytes,
        metadata_bytes,
        prefix_bytes,
        fuzzy_bytes,
        content_bytes,
        segment_bytes,
    ]
    .into_iter()
    .fold(0u64, u64::saturating_add);
    let bytes_per_record = if record_count == 0 {
        0
    } else {
        total_bytes / record_count as u64
    };
    let density_pressure = bytes_per_record > spec.density_policy.target_bytes_per_record;
    let plan = plan_content_segment_merge(&spec.content_segments, &spec.merge_policy)?;
    let reason = if spec.content_segments.is_empty() {
        CompactionReason::NoSegments
    } else if plan.tombstone_segments > 0 {
        CompactionReason::Tombstones
    } else if density_pressure && !plan.merge_segments.is_empty() {
        CompactionReason::IndexDensity
    } else if !plan.merge_segments.is_empty() {
        CompactionReason::TierPressure
    } else {
        CompactionReason::BelowThreshold
    };
    let action = compaction_action(reason, spec.compaction_pressure);
    let compaction = IndexCompactionSchedule {
        scheduled: !plan.merge_segments.is_empty() && action != CompactionAction::Defer,
        tier: plan.tier,
        merge_segments: plan.merge_segments,
        retained_segments: plan.retained_segments,
        merge_bytes: plan.merge_bytes,
        effective_max_merge_bytes: effective_compaction_bytes(
            spec.merge_policy.max_merge_bytes,
            action,
        ),
        tombstone_segments: plan.tombstone_segments,
        reason,
        action,
        pressure: spec.compaction_pressure,
    };

    Ok(IndexFootprintReport {
        record_count,
        record_bytes,
        column_count,
        column_bytes,
        column_string_pool_bytes,
        metadata_terms,
        metadata_bytes,
        prefix_keys,
        prefix_bytes,
        fuzzy_keys,
        fuzzy_bytes,
        content_archives,
        content_terms,
        content_bytes,
        segment_count,
        segment_bytes,
        segment_postings,
        tombstone_segments,
        tombstones,
        total_bytes,
        bytes_per_record,
        compaction,
    })
}

fn compaction_action(reason: CompactionReason, pressure: CompactionPressure) -> CompactionAction {
    if matches!(
        reason,
        CompactionReason::BelowThreshold | CompactionReason::NoSegments
    ) {
        return CompactionAction::Defer;
    }
    if matches!(pressure.io, IoPressure::Saturated)
        || matches!(pressure.thermal, ThermalState::Critical)
    {
        return CompactionAction::Defer;
    }
    if matches!(pressure.io, IoPressure::Elevated)
        || matches!(pressure.thermal, ThermalState::Serious)
        || matches!(pressure.battery, BatteryState::LowPower)
        || matches!(pressure.user_activity, UserActivity::Active)
    {
        return CompactionAction::Throttle;
    }
    CompactionAction::Run
}

fn effective_compaction_bytes(max_merge_bytes: u64, action: CompactionAction) -> u64 {
    match action {
        CompactionAction::Run => max_merge_bytes,
        CompactionAction::Throttle => (max_merge_bytes / 2).max(1 << 20),
        CompactionAction::Defer => 0,
    }
}

fn mapped_bytes(path: &Path, mapped_len: usize) -> Result<u64> {
    let mapped_len = u64::try_from(mapped_len)
        .map_err(|_| GfmError::Format(format!("mapped file {} is too large", path.display())))?;
    Ok(mapped_len)
}

impl LiveIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn indexed_records(&self) -> usize {
        self.index.len()
    }

    pub fn from_records(records: Vec<FileRecord>) -> Self {
        let mut live = Self::new();
        for record in records {
            live.index.insert(record);
        }
        live
    }

    pub fn from_records_with_columns(
        records: Vec<FileRecord>,
        columns: Vec<SearchRecordColumns>,
    ) -> (Self, usize) {
        let mut live = Self::new();
        let mut columns_by_id = columns
            .into_iter()
            .map(|columns| (columns.id, columns))
            .collect::<HashMap<_, _>>();
        let mut applied = 0usize;
        for record in records {
            if let Some(columns) = columns_by_id.remove(&record.id) {
                if live.index.insert_with_columns(record, columns) {
                    applied += 1;
                }
            } else {
                live.index.insert(record);
            }
        }
        (live, applied)
    }

    pub fn from_records_with_columns_and_fuzzy(
        records: Vec<FileRecord>,
        columns: Vec<SearchRecordColumns>,
        fuzzy: Vec<SearchFuzzyPosting>,
    ) -> (Self, usize, usize) {
        let mut live = Self::new();
        let mut columns_by_id = columns
            .into_iter()
            .map(|columns| (columns.id, columns))
            .collect::<HashMap<_, _>>();
        let mut applied = 0usize;
        for record in records {
            if let Some(columns) = columns_by_id.remove(&record.id) {
                if live
                    .index
                    .insert_with_columns_deferred_fuzzy(record, columns)
                {
                    applied += 1;
                }
            } else {
                live.index.insert(record);
            }
        }
        let fuzzy_keys = live.index.import_fuzzy_postings(&fuzzy);
        (live, applied, fuzzy_keys)
    }

    pub fn from_records_deferred_sidecars(records: Vec<FileRecord>) -> Self {
        let mut live = Self::new();
        for record in records {
            let columns = SearchRecordColumns {
                id: record.id,
                name: record.name.clone(),
                path: record.path.to_string_lossy().into_owned(),
                extension: record.extension().map(ToOwned::to_owned),
                tags: record.tags.clone(),
                comment: record.finder_comment.clone(),
            };
            live.index
                .insert_with_columns_deferred_sidecars(record, columns);
        }
        live
    }

    pub fn from_records_with_sidecars(
        records: Vec<FileRecord>,
        columns: Vec<SearchRecordColumns>,
        metadata: Vec<SearchMetadataPosting>,
        prefixes: Vec<SearchPrefixPosting>,
        fuzzy: Vec<SearchFuzzyPosting>,
        content: Vec<ContentPosting>,
    ) -> (Self, usize, usize, usize, usize, usize) {
        let mut live = Self::new();
        let mut columns_by_id = columns
            .into_iter()
            .map(|columns| (columns.id, columns))
            .collect::<HashMap<_, _>>();
        let mut applied = 0usize;
        for record in records {
            if let Some(columns) = columns_by_id.remove(&record.id) {
                if live
                    .index
                    .insert_with_columns_deferred_sidecars(record, columns)
                {
                    applied += 1;
                }
            } else {
                live.index.insert(record);
            }
        }
        let metadata_keys = live.index.import_metadata_postings(&metadata);
        let prefix_keys = live.index.import_prefix_postings(&prefixes);
        let fuzzy_keys = live.index.import_fuzzy_postings(&fuzzy);
        let content_keys = content.len();
        live.index.import_content_postings(&content);
        (
            live,
            applied,
            metadata_keys,
            prefix_keys,
            fuzzy_keys,
            content_keys,
        )
    }

    pub fn apply_record_columns(&mut self, columns: SearchRecordColumns) -> bool {
        self.index.apply_record_columns(columns)
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        self.index.query(query, limit)
    }

    pub fn search_with_lookup(
        &self,
        query: &str,
        limit: usize,
        lookup: &dyn SearchLookup,
    ) -> Result<Vec<SearchHit>> {
        self.index.query_structured_with_lookup_cancellable(
            &SearchQuery::parse(query),
            limit,
            lookup,
            &Cancellation::default(),
        )
    }

    pub fn search_with_lookup_budget(
        &self,
        query: &str,
        limit: usize,
        lookup: &dyn SearchLookup,
        budget: SearchLookupBudget,
    ) -> Result<SearchQueryReport> {
        let cache_before = lookup.cache_telemetry();
        let mut report = self.index.query_structured_with_lookup_budget_cancellable(
            &SearchQuery::parse(query),
            limit,
            lookup,
            budget,
            &Cancellation::default(),
        )?;
        let cache_after = lookup.cache_telemetry();
        report.lookup.merge_cache_delta(&cache_before, &cache_after);
        Ok(report)
    }

    pub fn stream_search(&self, query: &str, limit: usize) -> Result<Vec<SearchStreamBatch>> {
        self.index.stream(query, limit)
    }

    pub fn search_cancellable(
        &self,
        query: &str,
        limit: usize,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchHit>> {
        self.index.query_cancellable(query, limit, cancellation)
    }

    pub fn search_with_snippets(
        &self,
        query: &str,
        limit: usize,
        extractor: &Extractor,
        context_bytes: usize,
    ) -> Result<Vec<SearchHit>> {
        let parsed = SearchQuery::parse(query);
        let mut hits = self.index.query_structured(&parsed, limit);
        for hit in &mut hits {
            if matches!(hit.reason, gfm_types::MatchReason::Content) {
                hit.snippet = extractor.snippet_for_record(
                    &hit.record,
                    &parsed.terms,
                    &parsed.phrases,
                    context_bytes,
                )?;
            }
        }
        Ok(hits)
    }

    pub fn index_content(&mut self, extractor: &Extractor) -> Result<usize> {
        let records: Vec<_> = self.index.records().cloned().collect();
        let mut indexed = 0;
        for record in records {
            if let Some(document) = extractor.extract_record(&record)? {
                self.index.insert_content(record.id, &document.text);
                indexed += 1;
            }
        }
        Ok(indexed)
    }

    pub fn save_content_postings(&self, path: impl AsRef<Path>) -> Result<()> {
        write_content_postings(path, &self.index.content_postings())
    }

    pub fn content_postings(&self) -> Vec<gfm_types::ContentPosting> {
        self.index.content_postings()
    }

    pub fn load_content_postings(&mut self, path: impl AsRef<Path>) -> Result<usize> {
        let postings = read_content_postings(path)?;
        let terms = postings.len();
        self.index.import_content_postings(&postings);
        Ok(terms)
    }

    pub fn load_content_set_postings(
        &mut self,
        paths: &[impl AsRef<Path>],
        query: &str,
    ) -> Result<usize> {
        let content = MmapContentSet::open(paths)?;
        let postings = content.postings_for_terms(content_query_terms(query))?;
        let terms = postings.len();
        self.index.import_content_postings(&postings);
        Ok(terms)
    }

    pub fn load_content_manifest_postings(
        &mut self,
        manifest_path: impl AsRef<Path>,
        query: &str,
    ) -> Result<usize> {
        let content = MmapContentSet::open_manifest(manifest_path)?;
        let postings = content.postings_for_terms(content_query_terms(query))?;
        let terms = postings.len();
        self.index.import_content_postings(&postings);
        Ok(terms)
    }

    pub fn apply_event(&mut self, event: &FileEvent) -> Result<UpdateOutcome> {
        match &event.kind {
            FileEventKind::Create | FileEventKind::Other => self.upsert_path(&event.path),
            FileEventKind::Modify => {
                let report = self.apply_metadata_update(&event.path)?;
                Ok(UpdateOutcome::MetadataUpdated {
                    changed: report.changed.len(),
                })
            }
            FileEventKind::Remove => {
                let removed = self.index.remove_subtree(&event.path).len();
                Ok(UpdateOutcome::Removed { records: removed })
            }
            FileEventKind::Rename { from, to } => {
                let report = self.apply_rename(from, to)?;
                Ok(UpdateOutcome::Renamed {
                    removed: report.removed,
                    inserted: report.inserted,
                })
            }
            FileEventKind::Rescan => Ok(UpdateOutcome::NeedsRescan),
        }
    }

    pub fn apply_rename(&mut self, from: &Path, to: &Path) -> Result<RenameCorrelationReport> {
        correlate_rename(&mut self.index, from, to)
    }

    pub fn apply_metadata_update(&mut self, path: &Path) -> Result<MetadataUpdateReport> {
        let previous = self.index.get_path(path).cloned();
        let current =
            gfm_fs::record_for_path(path, previous.as_ref().and_then(|r| r.parent), false)?;
        let report = MetadataUpdateReport::from_records(path, previous.as_ref(), &current);
        self.index.insert(current);
        Ok(report)
    }

    fn upsert_path(&mut self, path: &Path) -> Result<UpdateOutcome> {
        let record = gfm_fs::record_for_path(path, None, false)?;
        self.index.insert(record);
        Ok(UpdateOutcome::Upserted)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateOutcome {
    Upserted,
    MetadataUpdated { changed: usize },
    Removed { records: usize },
    Renamed { removed: usize, inserted: usize },
    NeedsRescan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentIndexOptions {
    pub batch_size: usize,
    pub segment_prefix: String,
}

impl Default for ContentIndexOptions {
    fn default() -> Self {
        Self {
            batch_size: 1024,
            segment_prefix: "content".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentIndexReport {
    pub indexed: usize,
    pub skipped: usize,
    pub terms: usize,
    pub segments: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContentMaintenanceOptions {
    pub merge_policy: ContentMergePolicy,
    pub cleanup_policy: ContentArchiveCleanupPolicy,
    pub cleanup_retired_archives: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentMaintenanceReport {
    pub scheduled: bool,
    pub terms: usize,
    pub merged_segments: Vec<PathBuf>,
    pub retained_segments: Vec<PathBuf>,
    pub published_archive: Option<PathBuf>,
    pub tier: ContentMergeTier,
    pub merge_bytes: u64,
    pub tombstone_segments: usize,
    pub manifest_archives: usize,
    pub removed_archives: Vec<PathBuf>,
    pub active_archives: Vec<PathBuf>,
    pub missing_archives: Vec<PathBuf>,
    pub cleanup_action: ContentArchiveCleanupAction,
    pub cleanup_bytes: u64,
    pub deferred_archives: Vec<PathBuf>,
    pub deferred_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentIndexJobSpec {
    pub root: PathBuf,
    pub segment_dir: PathBuf,
    pub records_path: PathBuf,
    pub content_path: PathBuf,
    pub batch_size: usize,
}

impl ContentIndexJobSpec {
    pub fn new(
        root: impl Into<PathBuf>,
        segment_dir: impl Into<PathBuf>,
        records_path: impl Into<PathBuf>,
        content_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            root: root.into(),
            segment_dir: segment_dir.into(),
            records_path: records_path.into(),
            content_path: content_path.into(),
            batch_size: ContentIndexOptions::default().batch_size,
        }
    }

    pub fn options(&self) -> ContentIndexOptions {
        ContentIndexOptions {
            batch_size: self.batch_size,
            segment_prefix: ContentIndexOptions::default().segment_prefix,
        }
    }

    pub fn write(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let file = fs::File::create(path).map_err(|err| GfmError::io(path, err))?;
        let mut writer = BufWriter::new(file);
        writeln!(writer, "gfm-content-job-v1").map_err(|err| GfmError::io(path, err))?;
        writeln!(writer, "root\t{}", escape_path(&self.root))
            .map_err(|err| GfmError::io(path, err))?;
        writeln!(writer, "segment_dir\t{}", escape_path(&self.segment_dir))
            .map_err(|err| GfmError::io(path, err))?;
        writeln!(writer, "records_path\t{}", escape_path(&self.records_path))
            .map_err(|err| GfmError::io(path, err))?;
        writeln!(writer, "content_path\t{}", escape_path(&self.content_path))
            .map_err(|err| GfmError::io(path, err))?;
        writeln!(writer, "batch_size\t{}", self.batch_size)
            .map_err(|err| GfmError::io(path, err))?;
        writer.flush().map_err(|err| GfmError::io(path, err))
    }

    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = fs::File::open(path).map_err(|err| GfmError::io(path, err))?;
        let mut lines = BufReader::new(file).lines();
        match lines.next() {
            Some(Ok(header)) if header == "gfm-content-job-v1" => {}
            Some(Ok(header)) => {
                return Err(GfmError::Format(format!(
                    "unsupported content job header `{header}` in {}",
                    path.display()
                )))
            }
            Some(Err(err)) => return Err(GfmError::io(path, err)),
            None => {
                return Err(GfmError::Format(format!(
                    "empty content job {}",
                    path.display()
                )))
            }
        }

        let mut root = None;
        let mut segment_dir = None;
        let mut records_path = None;
        let mut content_path = None;
        let mut batch_size = None;
        for (line_index, line) in lines.enumerate() {
            let line = line.map_err(|err| GfmError::io(path, err))?;
            let (key, value) = line.split_once('\t').ok_or_else(|| {
                GfmError::Format(format!(
                    "{} line {}: expected key and value",
                    path.display(),
                    line_index + 2
                ))
            })?;
            match key {
                "root" => root = Some(PathBuf::from(unescape(value)?)),
                "segment_dir" => segment_dir = Some(PathBuf::from(unescape(value)?)),
                "records_path" => records_path = Some(PathBuf::from(unescape(value)?)),
                "content_path" => content_path = Some(PathBuf::from(unescape(value)?)),
                "batch_size" => {
                    batch_size = Some(value.parse().map_err(|err| {
                        GfmError::Format(format!("invalid content job batch size `{value}`: {err}"))
                    })?)
                }
                other => {
                    return Err(GfmError::Format(format!(
                        "{}: unknown content job field `{other}`",
                        path.display()
                    )))
                }
            }
        }

        Ok(Self {
            root: required_field(root, "root", path)?,
            segment_dir: required_field(segment_dir, "segment_dir", path)?,
            records_path: required_field(records_path, "records_path", path)?,
            content_path: required_field(content_path, "content_path", path)?,
            batch_size: required_field(batch_size, "batch_size", path)?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct BackgroundContentIndexer {
    extractor: Extractor,
    options: ContentIndexOptions,
}

impl BackgroundContentIndexer {
    pub fn new(extractor: Extractor, options: ContentIndexOptions) -> Self {
        Self { extractor, options }
    }

    pub fn run_to_segments(
        &self,
        snapshot: &IndexSnapshot,
        output_dir: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<ContentIndexReport> {
        let output_dir = output_dir.as_ref();
        fs::create_dir_all(output_dir).map_err(|err| gfm_types::GfmError::io(output_dir, err))?;
        let batch_size = self.options.batch_size.max(1);
        let mut report = ContentIndexReport {
            indexed: 0,
            skipped: 0,
            terms: 0,
            segments: Vec::new(),
        };

        for (batch_index, records) in snapshot.records.chunks(batch_size).enumerate() {
            cancellation.check()?;
            let segment_path = output_dir.join(format!(
                "{}-{:08}.gfmseg",
                self.options.segment_prefix, batch_index
            ));
            let mut live = LiveIndex::from_records(records.to_vec());
            let indexed = live.index_content(&self.extractor)?;
            report.indexed += indexed;
            report.skipped += records.len().saturating_sub(indexed);
            let postings = live.content_postings();
            report.terms += postings.len();
            write_content_segment(
                &segment_path,
                &ContentSegment {
                    tombstones: Vec::new(),
                    postings,
                },
            )?;
            report.segments.push(segment_path);
        }
        Ok(report)
    }

    pub fn run_and_compact(
        &self,
        snapshot: &IndexSnapshot,
        segment_dir: impl AsRef<Path>,
        content_path: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<ContentIndexReport> {
        let mut report = self.run_to_segments(snapshot, segment_dir, cancellation)?;
        cancellation.check()?;
        report.terms = compact_content_segments(content_path, &report.segments)?.len();
        Ok(report)
    }

    pub fn maintain_segments(
        &self,
        manifest_path: impl AsRef<Path>,
        output_archive: impl AsRef<Path>,
        segments: &[impl AsRef<Path>],
        options: &ContentMaintenanceOptions,
    ) -> Result<ContentMaintenanceReport> {
        let manifest_path = manifest_path.as_ref();
        let output_archive = output_archive.as_ref();
        let plan = plan_content_segment_merge(segments, &options.merge_policy)?;
        if plan.merge_segments.is_empty() {
            return Ok(ContentMaintenanceReport {
                scheduled: false,
                terms: 0,
                merged_segments: Vec::new(),
                retained_segments: plan.retained_segments,
                published_archive: None,
                tier: plan.tier,
                merge_bytes: plan.merge_bytes,
                tombstone_segments: plan.tombstone_segments,
                manifest_archives: ContentArchiveManifest::read(manifest_path)?.archives.len(),
                removed_archives: Vec::new(),
                active_archives: Vec::new(),
                missing_archives: Vec::new(),
                cleanup_action: ContentArchiveCleanupAction::Skip,
                cleanup_bytes: 0,
                deferred_archives: Vec::new(),
                deferred_bytes: 0,
            });
        }

        let outcome = compact_content_segments_with_policy(
            output_archive,
            &plan.merge_segments,
            &options.merge_policy,
        )?;
        let manifest = ContentArchiveManifest::read(manifest_path)?;
        let promotion = manifest.promote_archive(
            manifest_path,
            ContentArchiveManifestEntry {
                tier: outcome.tier,
                path: output_archive.to_path_buf(),
            },
            &[] as &[PathBuf],
        )?;
        promotion.manifest.write(manifest_path)?;
        let cleanup_plan = promotion.manifest.plan_inactive_archive_cleanup(
            manifest_path,
            &promotion.retired_archives,
            &options.cleanup_policy,
        )?;
        let cleanup = if options.cleanup_retired_archives
            && cleanup_plan.action == ContentArchiveCleanupAction::Cleanup
        {
            promotion
                .manifest
                .cleanup_inactive_archives(manifest_path, &cleanup_plan.cleanup_archives)?
        } else {
            gfm_store::ContentArchiveCleanupReport {
                removed_archives: Vec::new(),
                active_archives: Vec::new(),
                missing_archives: Vec::new(),
            }
        };
        Ok(ContentMaintenanceReport {
            scheduled: true,
            terms: outcome.postings.len(),
            merged_segments: outcome.merged_segments,
            retained_segments: outcome.retained_segments,
            published_archive: Some(output_archive.to_path_buf()),
            tier: outcome.tier,
            merge_bytes: outcome.merge_bytes,
            tombstone_segments: outcome.tombstone_segments,
            manifest_archives: promotion.manifest.archives.len(),
            removed_archives: cleanup.removed_archives,
            active_archives: cleanup.active_archives,
            missing_archives: cleanup.missing_archives,
            cleanup_action: cleanup_plan.action,
            cleanup_bytes: cleanup_plan.cleanup_bytes,
            deferred_archives: cleanup_plan.deferred_archives,
            deferred_bytes: cleanup_plan.deferred_bytes,
        })
    }
}

impl Default for BackgroundContentIndexer {
    fn default() -> Self {
        Self::new(Extractor::default(), ContentIndexOptions::default())
    }
}

#[derive(Debug, Clone)]
pub struct Indexer {
    options: ScanOptions,
}

impl Indexer {
    pub fn new(options: ScanOptions) -> Self {
        Self { options }
    }

    pub fn build(&self, root: impl AsRef<Path>) -> Result<IndexSnapshot> {
        scan_tree(root, self.options.clone()).map(IndexSnapshot::from_page)
    }

    pub fn build_fair(
        &self,
        root: impl AsRef<Path>,
        visible_roots: &[PathBuf],
        visible_burst: usize,
    ) -> Result<FairScanReport> {
        FairScanScheduler::new(self.options.clone(), visible_burst).scan(root, visible_roots)
    }

    pub fn build_persistent(
        &self,
        root: impl AsRef<Path>,
        records_path: impl AsRef<Path>,
        state_path: impl AsRef<Path>,
    ) -> Result<IndexVolumeState> {
        let records_path = records_path.as_ref();
        let state_path = state_path.as_ref();
        let previous = state_path
            .exists()
            .then(|| IndexVolumeState::read(state_path))
            .transpose()?;
        let snapshot = self.build(root)?;
        snapshot.save(records_path)?;
        let state = snapshot.volume_state(records_path.to_path_buf(), previous.as_ref())?;
        state.write(state_path)?;
        Ok(state)
    }

    pub fn plan_persistent_recovery(
        &self,
        root: impl AsRef<Path>,
        records_path: impl AsRef<Path>,
        state_path: impl AsRef<Path>,
    ) -> PersistentIndexPlan {
        plan_persistent_index_recovery(root, records_path, state_path)
    }

    pub fn recover_persistent(
        &self,
        root: impl AsRef<Path>,
        records_path: impl AsRef<Path>,
        state_path: impl AsRef<Path>,
        quarantine_dir: impl AsRef<Path>,
    ) -> Result<PersistentIndexRecovery> {
        let root = root.as_ref().to_path_buf();
        let records_path = records_path.as_ref().to_path_buf();
        let state_path = state_path.as_ref().to_path_buf();
        recovery::recover_persistent_index(
            &root,
            &records_path,
            &state_path,
            quarantine_dir,
            || self.build_persistent(&root, &records_path, &state_path),
        )
    }

    pub fn build_with_progress(
        &self,
        root: impl AsRef<Path>,
        records_path: impl AsRef<Path>,
        progress_path: impl AsRef<Path>,
    ) -> Result<ScanProgressCheckpoint> {
        let root = root.as_ref();
        let records_path = records_path.as_ref();
        let progress_path = progress_path.as_ref();
        let started = ScanProgressCheckpoint::started(root, records_path);
        started.write(progress_path)?;
        let snapshot = self.build(root)?;
        let last_path = snapshot.records.last().map(|record| record.path.clone());
        let scanned = snapshot.records.len();
        let inaccessible = snapshot.inaccessible.len();
        let progress = started
            .with_progress(scanned, inaccessible, last_path)
            .with_publication(1, 0);
        snapshot.save(records_path)?;
        let completed = progress.completed();
        completed.write(progress_path)?;
        Ok(completed)
    }

    pub fn scan_progress(&self, progress_path: impl AsRef<Path>) -> Result<ScanProgressCheckpoint> {
        ScanProgressCheckpoint::read(progress_path)
    }

    pub fn checkpoint_fsevents_cursor(
        &self,
        state_path: impl AsRef<Path>,
        cursor_path: impl AsRef<Path>,
        last_event_id: u64,
        health: FseventsCursorHealth,
    ) -> Result<FseventsCursor> {
        let volume = IndexVolumeState::read(state_path)?;
        let cursor = FseventsCursor::checkpoint(&volume, last_event_id, health);
        cursor.write(cursor_path)?;
        Ok(cursor)
    }

    pub fn fsevents_resume_plan(
        &self,
        state_path: impl AsRef<Path>,
        cursor_path: impl AsRef<Path>,
    ) -> Result<FseventsResumePlan> {
        let volume = IndexVolumeState::read(state_path)?;
        FseventsResumePlan::read(&volume, cursor_path)
    }

    pub fn repair_schedule(
        &self,
        state_path: impl AsRef<Path>,
        cursor_path: impl AsRef<Path>,
        observed_event_ids: &[u64],
        dropped_roots: &[PathBuf],
        explicit_reason: Option<&str>,
    ) -> Result<RepairSchedule> {
        let volume = IndexVolumeState::read(state_path)?;
        let resume = FseventsResumePlan::read(&volume, cursor_path)?;
        Ok(RepairSchedule::evaluate(
            &volume,
            resume,
            observed_event_ids,
            dropped_roots,
            explicit_reason,
        ))
    }

    pub fn load(&self, path: impl AsRef<Path>) -> Result<IndexSnapshot> {
        Ok(IndexSnapshot {
            root: PathBuf::new(),
            records: read_records(path)?,
            inaccessible: Vec::new(),
        })
    }

    pub fn load_live_with_content(
        &self,
        records_path: impl AsRef<Path>,
        content_path: impl AsRef<Path>,
    ) -> Result<LiveIndex> {
        let mut live = self.load(records_path)?.into_live();
        live.load_content_postings(content_path)?;
        Ok(live)
    }

    pub fn load_live_with_content_set(
        &self,
        records_path: impl AsRef<Path>,
        content_paths: &[impl AsRef<Path>],
        query: &str,
    ) -> Result<(LiveIndex, usize)> {
        let mut live = self.load(records_path)?.into_live();
        let terms = live.load_content_set_postings(content_paths, query)?;
        Ok((live, terms))
    }

    pub fn load_live_with_content_manifest(
        &self,
        records_path: impl AsRef<Path>,
        manifest_path: impl AsRef<Path>,
        query: &str,
    ) -> Result<(LiveIndex, usize)> {
        let mut live = self.load(records_path)?.into_live();
        let terms = live.load_content_manifest_postings(manifest_path, query)?;
        Ok((live, terms))
    }

    pub fn compact_content_segments(
        &self,
        output: impl AsRef<Path>,
        segments: &[impl AsRef<Path>],
    ) -> Result<usize> {
        compact_content_segments(output, segments).map(|postings| postings.len())
    }

    pub fn compact_content_segments_with_policy(
        &self,
        output: impl AsRef<Path>,
        segments: &[impl AsRef<Path>],
        policy: &ContentMergePolicy,
    ) -> Result<ContentMergeOutcome> {
        compact_content_segments_with_policy(output, segments, policy)
    }
}

impl Default for Indexer {
    fn default() -> Self {
        Self::new(ScanOptions::default())
    }
}

fn required_field<T>(value: Option<T>, field: &str, path: &Path) -> Result<T> {
    value.ok_or_else(|| {
        GfmError::Format(format!(
            "{}: missing content job field `{field}`",
            path.display()
        ))
    })
}

fn escape_path(path: &Path) -> String {
    escape(&path.to_string_lossy())
}

fn escape(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' => output.push_str("\\\\"),
            '\t' => output.push_str("\\t"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            other => output.push(other),
        }
    }
    output
}

fn unescape(input: &str) -> Result<String> {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        match chars.next() {
            Some('\\') => output.push('\\'),
            Some('t') => output.push('\t'),
            Some('n') => output.push('\n'),
            Some('r') => output.push('\r'),
            Some(other) => {
                return Err(GfmError::Format(format!(
                    "invalid content job escape `\\{other}`"
                )))
            }
            None => return Err(GfmError::Format("trailing content job escape".to_string())),
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests;
