use gfm_content::Extractor;
use gfm_fs::{scan_tree, scan_tree_checked, ScanOptions};
use gfm_jobs::Cancellation;
pub use gfm_search::substring_candidate_grams;
pub use gfm_search::{
    SearchFuzzyPosting, SearchLookup, SearchLookupBudget, SearchLookupIds, SearchLookupTelemetry,
    SearchLookupTerms, SearchMetadataField, SearchMetadataPosting, SearchPrefixPosting,
    SearchQuery, SearchQueryReport, SearchRecordColumns, SearchStreamStage, SearchSubstringPosting,
    SearchVolumeScope,
};
use gfm_search::{SearchStreamBatch, ShardedSearchIndex};
use gfm_store::{
    compact_content_segments, compact_content_segments_with_policy, read_records_checked,
    write_records, write_records_checked, MmapContentArchive, MmapContentSet, MmapRecordArchive,
};
pub use gfm_store::{
    ContentArchiveCleanupAction, ContentArchiveCleanupPlan, ContentArchiveCleanupPolicy,
    ContentArchiveCleanupReport, ContentArchiveManifest, ContentArchiveManifestEntry,
    ContentManifestPromotion, ContentMergeOutcome, ContentMergePolicy, ContentMergeTier,
};
use gfm_types::{
    ContentSegment, DirectoryPage, FileId, FileRecord, GfmError, Result, ScanIssue, SearchHit,
};
use std::path::{Path, PathBuf};

use gfm_store::{write_content_segment, write_content_segment_checked};
#[cfg(test)]
use gfm_store::{MmapRecordColumns, MmapSubstringArchive};
#[cfg(test)]
use gfm_types::{FileEvent, FileEventKind};

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
mod session;
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
    inspect_index_footprint, inspect_index_footprint_checked, BatteryState, CompactionAction,
    CompactionPressure, CompactionReason, IndexCompactionSchedule, IndexDensityPolicy,
    IndexFootprintReport, IndexFootprintSpec, IoPressure, ThermalState, UserActivity,
};
pub use live::{LiveIndex, UpdateOutcome};
pub use lookup::{
    query_sidecar_imports, query_sidecar_imports_cancellable, ContentQueryLoadReport,
    SearchArchiveLookup, SidecarIndexQuerySession, SidecarQueryImport, SidecarQueryImportReport,
    SidecarQuerySessionReport, SidecarRecordHydrationReport,
};
pub use metadata::{
    diff_metadata, publish_secondary_metadata, MetadataUpdateReport,
    ProviderMetadataInvalidationReport, SecondaryMetadataPublicationReport,
};
pub use progress::{ScanProgressCheckpoint, SCAN_PROGRESS_SCHEMA_VERSION};
pub use recovery::{
    persistent_index_action_name, persistent_index_reason_name, plan_persistent_index_recovery,
    plan_persistent_index_recovery_cancellable, PersistentIndexAction, PersistentIndexPlan,
    PersistentIndexReason, PersistentIndexRecovery,
};
pub use rename::{correlate_rename, correlate_rename_checked, RenameCorrelationReport};
pub use repair::{RepairPriority, RepairReason, RepairSchedule, SubtreeRepairJob};
pub use scan::{FairScanReport, FairScanScheduler, FairScanSummary, ScanLane};
pub use session::{ContentIndexQuerySession, ContentQuerySessionReport};
pub use state::{IndexVolumeState, INDEX_STATE_SCHEMA_VERSION};
pub use volume::{
    parse_volume_indexing_policy, volume_indexing_policy_name, IndexMountState, IndexVolumeClass,
    IndexVolumeDescriptor, IndexVolumeEventKind, VolumeEventIndexInvalidationReport,
    VolumeIndexAction, VolumeIndexDecision, VolumeIndexPlan, VolumeIndexPolicy,
    VolumeIndexThrottle, VolumeInvalidationReport, VolumeThrottleClass,
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
pub struct IndexQuerySession {
    live: LiveIndex,
}

impl IndexQuerySession {
    pub fn from_records(records: Vec<FileRecord>) -> Self {
        Self {
            live: LiveIndex::from_records(records),
        }
    }

    pub fn from_snapshot(snapshot: IndexSnapshot) -> Self {
        Self::from_records(snapshot.records)
    }

    pub fn from_live(live: LiveIndex) -> Self {
        Self { live }
    }

    pub fn indexed_records(&self) -> usize {
        self.live.indexed_records()
    }

    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        self.live.search(query, limit)
    }

    pub fn search_with_volume_scope(
        &self,
        query: &str,
        limit: usize,
        scope: &SearchVolumeScope,
    ) -> Result<Vec<SearchHit>> {
        self.live.search_with_volume_scope(query, limit, scope)
    }

    pub fn search_structured_with_volume_scope_cancellable(
        &self,
        query: &SearchQuery,
        limit: usize,
        scope: &SearchVolumeScope,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchHit>> {
        self.live
            .search_structured_with_volume_scope_cancellable(query, limit, scope, cancellation)
    }

    pub fn search_with_volume_plan(
        &self,
        query: &str,
        limit: usize,
        plan: &VolumeIndexPlan,
    ) -> Result<Vec<SearchHit>> {
        self.search_with_volume_scope(query, limit, &plan.included_volume_scope())
    }

    pub fn stream_search(&self, query: &str, limit: usize) -> Result<Vec<SearchStreamBatch>> {
        self.live.stream_search(query, limit)
    }

    pub fn stream_search_cancellable(
        &self,
        query: &str,
        limit: usize,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchStreamBatch>> {
        self.live
            .stream_search_cancellable(query, limit, cancellation)
    }

    pub fn stream_structured_search(
        &self,
        query: &SearchQuery,
        limit: usize,
    ) -> Result<Vec<SearchStreamBatch>> {
        self.live.stream_structured_search(query, limit)
    }

    pub fn stream_structured_search_cancellable(
        &self,
        query: &SearchQuery,
        limit: usize,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchStreamBatch>> {
        self.live
            .stream_structured_search_cancellable(query, limit, cancellation)
    }

    pub fn stream_search_with_volume_scope(
        &self,
        query: &str,
        limit: usize,
        scope: &SearchVolumeScope,
    ) -> Result<Vec<SearchStreamBatch>> {
        self.live
            .stream_search_with_volume_scope(query, limit, scope)
    }

    pub fn stream_structured_search_with_volume_scope_cancellable(
        &self,
        query: &SearchQuery,
        limit: usize,
        scope: &SearchVolumeScope,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchStreamBatch>> {
        self.live
            .stream_structured_search_with_volume_scope_cancellable(
                query,
                limit,
                scope,
                cancellation,
            )
    }

    pub fn stream_search_with_volume_scope_cancellable(
        &self,
        query: &str,
        limit: usize,
        scope: &SearchVolumeScope,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchStreamBatch>> {
        self.live
            .stream_search_with_volume_scope_cancellable(query, limit, scope, cancellation)
    }

    pub fn stream_search_with_volume_plan(
        &self,
        query: &str,
        limit: usize,
        plan: &VolumeIndexPlan,
    ) -> Result<Vec<SearchStreamBatch>> {
        self.stream_search_with_volume_scope(query, limit, &plan.included_volume_scope())
    }

    pub fn search_cancellable(
        &self,
        query: &str,
        limit: usize,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchHit>> {
        self.live.search_cancellable(query, limit, cancellation)
    }
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

    pub fn stream_search_cancellable(
        &self,
        query: &str,
        limit: usize,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchStreamBatch>> {
        let mut index = ShardedSearchIndex::new();
        for record in self.records.iter().cloned() {
            cancellation.check()?;
            index.insert(record);
        }
        index.stream_cancellable(query, limit, cancellation)
    }

    pub fn stream_structured_search_cancellable(
        &self,
        query: &SearchQuery,
        limit: usize,
        cancellation: &Cancellation,
    ) -> Result<Vec<SearchStreamBatch>> {
        let mut index = ShardedSearchIndex::new();
        for record in self.records.iter().cloned() {
            cancellation.check()?;
            index.insert(record);
        }
        index.stream_structured_cancellable(query, limit, cancellation)
    }

    pub fn query_session(&self) -> IndexQuerySession {
        IndexQuerySession::from_records(self.records.clone())
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

    pub fn save_checked(
        &self,
        path: impl AsRef<Path>,
        check_control: impl FnMut() -> Result<()>,
    ) -> Result<()> {
        write_records_checked(path, &self.records, check_control)
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
        self.save_with_content_cancellable(
            records_path,
            content_path,
            extractor,
            &Cancellation::default(),
        )
    }

    pub fn save_with_content_cancellable(
        &self,
        records_path: impl AsRef<Path>,
        content_path: impl AsRef<Path>,
        extractor: &Extractor,
        cancellation: &Cancellation,
    ) -> Result<usize> {
        cancellation.check()?;
        self.save_checked(records_path, || cancellation.check())?;
        cancellation.check()?;
        let mut live = self.clone().into_live();
        let indexed = live.index_content_cancellable(extractor, cancellation)?;
        cancellation.check()?;
        live.save_content_postings_checked(content_path, || cancellation.check())?;
        cancellation.check()?;
        Ok(indexed)
    }

    pub fn save_content_segment(
        &self,
        segment_path: impl AsRef<Path>,
        extractor: &Extractor,
        tombstones: Vec<FileId>,
    ) -> Result<usize> {
        self.save_content_segment_cancellable(
            segment_path,
            extractor,
            tombstones,
            &Cancellation::default(),
        )
    }

    pub fn save_content_segment_cancellable(
        &self,
        segment_path: impl AsRef<Path>,
        extractor: &Extractor,
        tombstones: Vec<FileId>,
        cancellation: &Cancellation,
    ) -> Result<usize> {
        cancellation.check()?;
        let mut live = self.clone().into_live();
        let indexed = live.index_content_cancellable(extractor, cancellation)?;
        cancellation.check()?;
        let segment = ContentSegment {
            tombstones,
            postings: live.content_postings(),
        };
        write_content_segment_checked(segment_path, &segment, || cancellation.check())?;
        cancellation.check()?;
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

    pub fn build_cancellable(
        &self,
        root: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<IndexSnapshot> {
        scan_tree_checked(root, self.options.clone(), || cancellation.check())
            .map(IndexSnapshot::from_page)
    }

    pub fn build_fair(
        &self,
        root: impl AsRef<Path>,
        visible_roots: &[PathBuf],
        visible_burst: usize,
    ) -> Result<FairScanReport> {
        FairScanScheduler::new(self.options.clone(), visible_burst).scan(root, visible_roots)
    }

    pub fn build_fair_cancellable(
        &self,
        root: impl AsRef<Path>,
        visible_roots: &[PathBuf],
        visible_burst: usize,
        cancellation: &Cancellation,
    ) -> Result<FairScanReport> {
        FairScanScheduler::new(self.options.clone(), visible_burst).scan_cancellable(
            root,
            visible_roots,
            cancellation,
        )
    }

    pub fn build_persistent(
        &self,
        root: impl AsRef<Path>,
        records_path: impl AsRef<Path>,
        state_path: impl AsRef<Path>,
    ) -> Result<IndexVolumeState> {
        self.build_persistent_cancellable(root, records_path, state_path, &Cancellation::default())
    }

    pub fn build_persistent_cancellable(
        &self,
        root: impl AsRef<Path>,
        records_path: impl AsRef<Path>,
        state_path: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<IndexVolumeState> {
        let records_path = records_path.as_ref();
        let state_path = state_path.as_ref();
        cancellation.check()?;
        let previous = if state_path.try_exists().map_err(|err| {
            GfmError::io(
                state_path,
                format!("index state existence unavailable: {err}"),
            )
        })? {
            Some(IndexVolumeState::read_checked(state_path, || {
                cancellation.check()
            })?)
        } else {
            None
        };
        let snapshot = self.build_cancellable(root, cancellation)?;
        cancellation.check()?;
        snapshot.save_checked(records_path, || cancellation.check())?;
        cancellation.check()?;
        let state = snapshot.volume_state(records_path.to_path_buf(), previous.as_ref())?;
        cancellation.check()?;
        state.write_checked(state_path, || cancellation.check())?;
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

    pub fn plan_persistent_recovery_cancellable(
        &self,
        root: impl AsRef<Path>,
        records_path: impl AsRef<Path>,
        state_path: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<PersistentIndexPlan> {
        plan_persistent_index_recovery_cancellable(root, records_path, state_path, cancellation)
    }

    pub fn recover_persistent(
        &self,
        root: impl AsRef<Path>,
        records_path: impl AsRef<Path>,
        state_path: impl AsRef<Path>,
        quarantine_dir: impl AsRef<Path>,
    ) -> Result<PersistentIndexRecovery> {
        self.recover_persistent_cancellable(
            root,
            records_path,
            state_path,
            quarantine_dir,
            &Cancellation::default(),
        )
    }

    pub fn recover_persistent_cancellable(
        &self,
        root: impl AsRef<Path>,
        records_path: impl AsRef<Path>,
        state_path: impl AsRef<Path>,
        quarantine_dir: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<PersistentIndexRecovery> {
        let root = root.as_ref().to_path_buf();
        let records_path = records_path.as_ref().to_path_buf();
        let state_path = state_path.as_ref().to_path_buf();
        cancellation.check()?;
        recovery::recover_persistent_index_checked(
            &root,
            &records_path,
            &state_path,
            quarantine_dir,
            || self.build_persistent_cancellable(&root, &records_path, &state_path, cancellation),
            || cancellation.check(),
        )
    }

    pub fn build_with_progress(
        &self,
        root: impl AsRef<Path>,
        records_path: impl AsRef<Path>,
        progress_path: impl AsRef<Path>,
    ) -> Result<ScanProgressCheckpoint> {
        self.build_with_progress_cancellable(
            root,
            records_path,
            progress_path,
            &Cancellation::default(),
        )
    }

    pub fn build_with_progress_cancellable(
        &self,
        root: impl AsRef<Path>,
        records_path: impl AsRef<Path>,
        progress_path: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<ScanProgressCheckpoint> {
        let root = root.as_ref();
        let records_path = records_path.as_ref();
        let progress_path = progress_path.as_ref();
        cancellation.check()?;
        let started = ScanProgressCheckpoint::started(root, records_path);
        started.write_checked(progress_path, || cancellation.check())?;
        let snapshot = self.build_cancellable(root, cancellation)?;
        cancellation.check()?;
        let last_path = snapshot.records.last().map(|record| record.path.clone());
        let scanned = snapshot.records.len();
        let inaccessible = snapshot.inaccessible.len();
        let progress = started
            .with_progress(scanned, inaccessible, last_path)
            .with_publication(1, 0);
        cancellation.check()?;
        snapshot.save_checked(records_path, || cancellation.check())?;
        let completed = progress.completed();
        cancellation.check()?;
        completed.write_checked(progress_path, || cancellation.check())?;
        Ok(completed)
    }

    pub fn scan_progress(&self, progress_path: impl AsRef<Path>) -> Result<ScanProgressCheckpoint> {
        ScanProgressCheckpoint::read(progress_path)
    }

    pub fn scan_progress_cancellable(
        &self,
        progress_path: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<ScanProgressCheckpoint> {
        ScanProgressCheckpoint::read_checked(progress_path, || cancellation.check())
    }

    pub fn checkpoint_fsevents_cursor(
        &self,
        state_path: impl AsRef<Path>,
        cursor_path: impl AsRef<Path>,
        last_event_id: u64,
        health: FseventsCursorHealth,
    ) -> Result<FseventsCursor> {
        self.checkpoint_fsevents_cursor_cancellable(
            state_path,
            cursor_path,
            last_event_id,
            health,
            &Cancellation::default(),
        )
    }

    pub fn checkpoint_fsevents_cursor_cancellable(
        &self,
        state_path: impl AsRef<Path>,
        cursor_path: impl AsRef<Path>,
        last_event_id: u64,
        health: FseventsCursorHealth,
        cancellation: &Cancellation,
    ) -> Result<FseventsCursor> {
        cancellation.check()?;
        let volume = IndexVolumeState::read_checked(state_path, || cancellation.check())?;
        cancellation.check()?;
        let cursor = FseventsCursor::checkpoint(&volume, last_event_id, health);
        cursor.write_checked(cursor_path, || cancellation.check())?;
        cancellation.check()?;
        Ok(cursor)
    }

    pub fn fsevents_resume_plan(
        &self,
        state_path: impl AsRef<Path>,
        cursor_path: impl AsRef<Path>,
    ) -> Result<FseventsResumePlan> {
        self.fsevents_resume_plan_cancellable(state_path, cursor_path, &Cancellation::default())
    }

    pub fn fsevents_resume_plan_cancellable(
        &self,
        state_path: impl AsRef<Path>,
        cursor_path: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<FseventsResumePlan> {
        cancellation.check()?;
        let volume = IndexVolumeState::read_checked(state_path, || cancellation.check())?;
        cancellation.check()?;
        FseventsResumePlan::read_checked(&volume, cursor_path, || cancellation.check())
    }

    pub fn repair_schedule(
        &self,
        state_path: impl AsRef<Path>,
        cursor_path: impl AsRef<Path>,
        observed_event_ids: &[u64],
        dropped_roots: &[PathBuf],
        explicit_reason: Option<&str>,
    ) -> Result<RepairSchedule> {
        self.repair_schedule_cancellable(
            state_path,
            cursor_path,
            observed_event_ids,
            dropped_roots,
            explicit_reason,
            &Cancellation::default(),
        )
    }

    pub fn repair_schedule_cancellable(
        &self,
        state_path: impl AsRef<Path>,
        cursor_path: impl AsRef<Path>,
        observed_event_ids: &[u64],
        dropped_roots: &[PathBuf],
        explicit_reason: Option<&str>,
        cancellation: &Cancellation,
    ) -> Result<RepairSchedule> {
        cancellation.check()?;
        let volume = IndexVolumeState::read_checked(state_path, || cancellation.check())?;
        cancellation.check()?;
        let resume =
            FseventsResumePlan::read_checked(&volume, cursor_path, || cancellation.check())?;
        cancellation.check()?;
        RepairSchedule::evaluate_checked(
            &volume,
            resume,
            observed_event_ids,
            dropped_roots,
            explicit_reason,
            || cancellation.check(),
        )
    }

    pub fn load(&self, path: impl AsRef<Path>) -> Result<IndexSnapshot> {
        self.load_cancellable(path, &Cancellation::default())
    }

    pub fn load_cancellable(
        &self,
        path: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<IndexSnapshot> {
        cancellation.check()?;
        let records = read_records_checked(path, || cancellation.check())?;
        cancellation.check()?;
        Ok(IndexSnapshot {
            root: PathBuf::new(),
            records,
            inaccessible: Vec::new(),
        })
    }

    pub fn load_query_session(&self, path: impl AsRef<Path>) -> Result<IndexQuerySession> {
        self.load_query_session_cancellable(path, &Cancellation::default())
    }

    pub fn load_query_session_cancellable(
        &self,
        path: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<IndexQuerySession> {
        self.load_cancellable(path, cancellation)
            .map(IndexQuerySession::from_snapshot)
    }

    pub fn load_live_with_content(
        &self,
        records_path: impl AsRef<Path>,
        content_path: impl AsRef<Path>,
    ) -> Result<LiveIndex> {
        self.load_live_with_content_cancellable(
            records_path,
            content_path,
            &Cancellation::default(),
        )
    }

    pub fn load_live_with_content_cancellable(
        &self,
        records_path: impl AsRef<Path>,
        content_path: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<LiveIndex> {
        let mut live = self
            .load_cancellable(records_path, cancellation)?
            .into_live();
        cancellation.check()?;
        live.load_content_postings_cancellable(content_path, cancellation)?;
        Ok(live)
    }

    pub fn load_live_with_content_for_query(
        &self,
        records_path: impl AsRef<Path>,
        content_path: impl AsRef<Path>,
        query: &str,
    ) -> Result<(LiveIndex, ContentQueryLoadReport)> {
        self.load_live_with_content_for_query_cancellable(
            records_path,
            content_path,
            query,
            &Cancellation::default(),
        )
    }

    pub fn load_live_with_content_for_query_cancellable(
        &self,
        records_path: impl AsRef<Path>,
        content_path: impl AsRef<Path>,
        query: &str,
        cancellation: &Cancellation,
    ) -> Result<(LiveIndex, ContentQueryLoadReport)> {
        self.load_live_with_content_for_query_with_budget_cancellable(
            records_path,
            content_path,
            query,
            SearchLookupBudget::default(),
            cancellation,
        )
    }

    pub fn load_live_with_content_for_query_with_budget(
        &self,
        records_path: impl AsRef<Path>,
        content_path: impl AsRef<Path>,
        query: &str,
        budget: SearchLookupBudget,
    ) -> Result<(LiveIndex, ContentQueryLoadReport)> {
        self.load_live_with_content_for_query_with_budget_cancellable(
            records_path,
            content_path,
            query,
            budget,
            &Cancellation::default(),
        )
    }

    pub fn load_live_with_content_for_query_with_budget_cancellable(
        &self,
        records_path: impl AsRef<Path>,
        content_path: impl AsRef<Path>,
        query: &str,
        budget: SearchLookupBudget,
        cancellation: &Cancellation,
    ) -> Result<(LiveIndex, ContentQueryLoadReport)> {
        cancellation.check()?;
        let records = MmapRecordArchive::open_checked(records_path, || cancellation.check())?;
        cancellation.check()?;
        let content = MmapContentArchive::open_checked(content_path, || cancellation.check())?;
        cancellation.check()?;
        let postings = content.postings_for_terms_limit_checked(
            content_query_terms(query),
            budget.max_content_ids_per_term,
            || cancellation.check(),
        )?;
        cancellation.check()?;
        LiveIndex::from_mmap_records_with_content_postings_cancellable(
            &records,
            postings,
            cancellation,
        )
    }

    pub fn load_content_query_session(
        &self,
        records_path: impl AsRef<Path>,
        content_path: impl AsRef<Path>,
    ) -> Result<ContentIndexQuerySession> {
        self.load_content_query_session_cancellable(
            records_path,
            content_path,
            &Cancellation::default(),
        )
    }

    pub fn load_content_query_session_cancellable(
        &self,
        records_path: impl AsRef<Path>,
        content_path: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<ContentIndexQuerySession> {
        ContentIndexQuerySession::open_content_cancellable(records_path, content_path, cancellation)
    }

    pub fn load_content_set_query_session<I, P>(
        &self,
        records_path: impl AsRef<Path>,
        content_paths: I,
    ) -> Result<ContentIndexQuerySession>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        self.load_content_set_query_session_cancellable(
            records_path,
            content_paths,
            &Cancellation::default(),
        )
    }

    pub fn load_content_set_query_session_cancellable<I, P>(
        &self,
        records_path: impl AsRef<Path>,
        content_paths: I,
        cancellation: &Cancellation,
    ) -> Result<ContentIndexQuerySession>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        ContentIndexQuerySession::open_set_cancellable(records_path, content_paths, cancellation)
    }

    pub fn load_content_manifest_query_session(
        &self,
        records_path: impl AsRef<Path>,
        manifest_path: impl AsRef<Path>,
    ) -> Result<ContentIndexQuerySession> {
        self.load_content_manifest_query_session_cancellable(
            records_path,
            manifest_path,
            &Cancellation::default(),
        )
    }

    pub fn load_content_manifest_query_session_cancellable(
        &self,
        records_path: impl AsRef<Path>,
        manifest_path: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<ContentIndexQuerySession> {
        ContentIndexQuerySession::open_manifest_cancellable(
            records_path,
            manifest_path,
            cancellation,
        )
    }

    pub fn load_live_with_content_set(
        &self,
        records_path: impl AsRef<Path>,
        content_paths: &[impl AsRef<Path>],
        query: &str,
    ) -> Result<(LiveIndex, ContentQueryLoadReport)> {
        self.load_live_with_content_set_cancellable(
            records_path,
            content_paths,
            query,
            &Cancellation::default(),
        )
    }

    pub fn load_live_with_content_set_cancellable(
        &self,
        records_path: impl AsRef<Path>,
        content_paths: &[impl AsRef<Path>],
        query: &str,
        cancellation: &Cancellation,
    ) -> Result<(LiveIndex, ContentQueryLoadReport)> {
        cancellation.check()?;
        let records = MmapRecordArchive::open_checked(records_path, || cancellation.check())?;
        cancellation.check()?;
        let content = MmapContentSet::open_checked(content_paths, || cancellation.check())?;
        cancellation.check()?;
        let postings = content.postings_for_terms_limit_checked(
            content_query_terms(query),
            SearchLookupBudget::default().max_content_ids_per_term,
            || cancellation.check(),
        )?;
        cancellation.check()?;
        LiveIndex::from_mmap_records_with_content_postings_cancellable(
            &records,
            postings,
            cancellation,
        )
    }

    pub fn load_live_with_content_manifest(
        &self,
        records_path: impl AsRef<Path>,
        manifest_path: impl AsRef<Path>,
        query: &str,
    ) -> Result<(LiveIndex, ContentQueryLoadReport)> {
        self.load_live_with_content_manifest_cancellable(
            records_path,
            manifest_path,
            query,
            &Cancellation::default(),
        )
    }

    pub fn load_live_with_content_manifest_cancellable(
        &self,
        records_path: impl AsRef<Path>,
        manifest_path: impl AsRef<Path>,
        query: &str,
        cancellation: &Cancellation,
    ) -> Result<(LiveIndex, ContentQueryLoadReport)> {
        cancellation.check()?;
        let records = MmapRecordArchive::open_checked(records_path, || cancellation.check())?;
        cancellation.check()?;
        let content =
            MmapContentSet::open_manifest_checked(manifest_path, || cancellation.check())?;
        cancellation.check()?;
        let postings = content.postings_for_terms_limit_checked(
            content_query_terms(query),
            SearchLookupBudget::default().max_content_ids_per_term,
            || cancellation.check(),
        )?;
        cancellation.check()?;
        LiveIndex::from_mmap_records_with_content_postings_cancellable(
            &records,
            postings,
            cancellation,
        )
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
