use gfm_content::Extractor;
use gfm_fs::{scan_tree, ScanOptions};
use gfm_jobs::Cancellation;
pub use gfm_search::substring_candidate_grams;
pub use gfm_search::{
    SearchFuzzyPosting, SearchLookup, SearchLookupBudget, SearchLookupIds, SearchLookupTelemetry,
    SearchLookupTerms, SearchMetadataField, SearchMetadataPosting, SearchPrefixPosting,
    SearchQueryReport, SearchRecordColumns, SearchStreamStage, SearchSubstringPosting,
};
use gfm_search::{SearchQuery, SearchStreamBatch, ShardedSearchIndex};
use gfm_store::{
    compact_content_segments, compact_content_segments_with_policy, read_records, write_records,
    MmapContentArchive, MmapContentSet, MmapRecordArchive,
};
pub use gfm_store::{
    ContentArchiveCleanupAction, ContentArchiveCleanupPlan, ContentArchiveCleanupPolicy,
    ContentArchiveCleanupReport, ContentArchiveManifest, ContentArchiveManifestEntry,
    ContentManifestPromotion, ContentMergeOutcome, ContentMergePolicy, ContentMergeTier,
};
use gfm_types::{ContentSegment, DirectoryPage, FileId, FileRecord, Result, ScanIssue, SearchHit};
use std::path::{Path, PathBuf};

use gfm_store::write_content_segment;
#[cfg(test)]
use gfm_store::{MmapMetadataArchive, MmapRecordColumns, MmapSubstringArchive};
#[cfg(test)]
use gfm_types::{FileEvent, FileEventKind, GfmError};

mod backpressure;
mod content;
mod cursor;
mod footprint;
mod live;
mod lookup;
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
pub use content::{
    BackgroundContentIndexer, ContentIndexBatchReport, ContentIndexDelta, ContentIndexJobSpec,
    ContentIndexOptions, ContentIndexReport, ContentMaintenanceOptions, ContentMaintenanceReport,
    QuarantineContentIndexRequest,
};
pub use cursor::{
    FseventsCursor, FseventsCursorHealth, FseventsResumeAction, FseventsResumePlan,
    FSEVENTS_CURSOR_SCHEMA_VERSION,
};
pub use footprint::{
    inspect_index_footprint, BatteryState, CompactionAction, CompactionPressure, CompactionReason,
    IndexCompactionSchedule, IndexDensityPolicy, IndexFootprintReport, IndexFootprintSpec,
    IoPressure, ThermalState, UserActivity,
};
pub use live::{LiveIndex, UpdateOutcome};
pub use lookup::{
    query_sidecar_imports, ContentQueryLoadReport, SearchArchiveLookup, SidecarQueryImport,
    SidecarQueryImportReport, SidecarRecordHydrationReport,
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

    pub fn save_incremental_content_segment(
        &self,
        segment_path: impl AsRef<Path>,
        extractor: &Extractor,
        previous_records: &[FileRecord],
    ) -> Result<ContentIndexReport> {
        let delta = ContentIndexDelta::from_records(&self.records, previous_records);
        let mut live = LiveIndex::from_records(delta.records.clone());
        let indexed = live.index_content(extractor)?;
        let postings = live.content_postings();
        let terms = postings.len();
        write_content_segment(
            segment_path.as_ref(),
            &ContentSegment {
                tombstones: delta.tombstones.clone(),
                postings,
            },
        )?;
        Ok(ContentIndexReport {
            indexed,
            skipped: delta.records.len().saturating_sub(indexed),
            quarantined: 0,
            unchanged: delta.unchanged,
            tombstoned: delta.tombstones.len(),
            terms,
            segments: vec![segment_path.as_ref().to_path_buf()],
        })
    }

    pub fn into_live(self) -> LiveIndex {
        LiveIndex::from_records(self.records)
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

    pub fn load_live_with_content_for_query(
        &self,
        records_path: impl AsRef<Path>,
        content_path: impl AsRef<Path>,
        query: &str,
    ) -> Result<(LiveIndex, ContentQueryLoadReport)> {
        self.load_live_with_content_for_query_with_budget(
            records_path,
            content_path,
            query,
            SearchLookupBudget::default(),
        )
    }

    pub fn load_live_with_content_for_query_with_budget(
        &self,
        records_path: impl AsRef<Path>,
        content_path: impl AsRef<Path>,
        query: &str,
        budget: SearchLookupBudget,
    ) -> Result<(LiveIndex, ContentQueryLoadReport)> {
        let records = MmapRecordArchive::open(records_path)?;
        let content = MmapContentArchive::open(content_path)?;
        let postings = content.postings_for_terms_limit(
            content_query_terms(query),
            budget.max_content_ids_per_term,
        )?;
        LiveIndex::from_mmap_records_with_content_postings(&records, postings)
    }

    pub fn load_live_with_content_set(
        &self,
        records_path: impl AsRef<Path>,
        content_paths: &[impl AsRef<Path>],
        query: &str,
    ) -> Result<(LiveIndex, ContentQueryLoadReport)> {
        let records = MmapRecordArchive::open(records_path)?;
        let content = MmapContentSet::open(content_paths)?;
        let postings = content.postings_for_terms_limit(
            content_query_terms(query),
            SearchLookupBudget::default().max_content_ids_per_term,
        )?;
        LiveIndex::from_mmap_records_with_content_postings(&records, postings)
    }

    pub fn load_live_with_content_manifest(
        &self,
        records_path: impl AsRef<Path>,
        manifest_path: impl AsRef<Path>,
        query: &str,
    ) -> Result<(LiveIndex, ContentQueryLoadReport)> {
        let records = MmapRecordArchive::open(records_path)?;
        let content = MmapContentSet::open_manifest(manifest_path)?;
        let postings = content.postings_for_terms_limit(
            content_query_terms(query),
            SearchLookupBudget::default().max_content_ids_per_term,
        )?;
        LiveIndex::from_mmap_records_with_content_postings(&records, postings)
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

#[cfg(test)]
mod tests;
