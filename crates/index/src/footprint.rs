use gfm_store::{
    plan_content_segment_merge, summarize_content_segment, ContentMergePolicy, ContentMergeTier,
    MmapContentSet, MmapFuzzyArchive, MmapMetadataArchive, MmapPrefixArchive, MmapRecordArchive,
    MmapRecordColumns, MmapSubstringArchive,
};
use gfm_types::{GfmError, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexFootprintSpec {
    pub records: PathBuf,
    pub columns: Option<PathBuf>,
    pub metadata: Option<PathBuf>,
    pub prefixes: Option<PathBuf>,
    pub substrings: Option<PathBuf>,
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
            substrings: None,
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
    pub substring_keys: usize,
    pub substring_bytes: u64,
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

    let (substring_keys, substring_bytes) = if let Some(path) = &spec.substrings {
        let archive = MmapSubstringArchive::open(path)?;
        (
            archive.indexed_grams(),
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
        substring_bytes,
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
        substring_keys,
        substring_bytes,
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
