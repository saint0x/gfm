use gfm_mac_sys::{read_spotlight_attributes_batch, NativeSpotlightStatus};
use gfm_types::{
    FileId, FileKind, FileRecord, GfmError, Result, SecondaryMetadataRecord, VolumeId,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotlightStatus {
    Available,
    Missing,
    Unavailable,
}

impl SpotlightStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Missing => "missing",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SpotlightField {
    DisplayName,
    Kind,
    ContentType,
    FinderComment,
    UserTags,
    Authors,
    WhereFroms,
    LastUsedDate,
}

impl SpotlightField {
    pub const fn key(self) -> &'static str {
        match self {
            Self::DisplayName => "kMDItemDisplayName",
            Self::Kind => "kMDItemKind",
            Self::ContentType => "kMDItemContentType",
            Self::FinderComment => "kMDItemFinderComment",
            Self::UserTags => "kMDItemUserTags",
            Self::Authors => "kMDItemAuthors",
            Self::WhereFroms => "kMDItemWhereFroms",
            Self::LastUsedDate => "kMDItemLastUsedDate",
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DisplayName => "display-name",
            Self::Kind => "kind",
            Self::ContentType => "content-type",
            Self::FinderComment => "finder-comment",
            Self::UserTags => "user-tags",
            Self::Authors => "authors",
            Self::WhereFroms => "where-froms",
            Self::LastUsedDate => "last-used-date",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        match key {
            "kMDItemDisplayName" => Some(Self::DisplayName),
            "kMDItemKind" => Some(Self::Kind),
            "kMDItemContentType" => Some(Self::ContentType),
            "kMDItemFinderComment" => Some(Self::FinderComment),
            "kMDItemUserTags" => Some(Self::UserTags),
            "kMDItemAuthors" => Some(Self::Authors),
            "kMDItemWhereFroms" => Some(Self::WhereFroms),
            "kMDItemLastUsedDate" => Some(Self::LastUsedDate),
            _ => None,
        }
    }
}

const SPOTLIGHT_FIELDS: [SpotlightField; 8] = [
    SpotlightField::DisplayName,
    SpotlightField::Kind,
    SpotlightField::ContentType,
    SpotlightField::FinderComment,
    SpotlightField::UserTags,
    SpotlightField::Authors,
    SpotlightField::WhereFroms,
    SpotlightField::LastUsedDate,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotlightSnapshot {
    pub path: PathBuf,
    pub status: SpotlightStatus,
    pub attributes: BTreeMap<SpotlightField, Vec<String>>,
    pub reason: Option<String>,
}

impl SpotlightSnapshot {
    pub fn available(
        path: impl Into<PathBuf>,
        attributes: BTreeMap<SpotlightField, Vec<String>>,
    ) -> Self {
        Self {
            path: path.into(),
            status: SpotlightStatus::Available,
            attributes,
            reason: None,
        }
    }

    pub fn missing(path: impl Into<PathBuf>, reason: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            status: SpotlightStatus::Missing,
            attributes: BTreeMap::new(),
            reason: Some(reason.into()),
        }
    }

    pub fn unavailable(path: impl Into<PathBuf>, reason: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            status: SpotlightStatus::Unavailable,
            attributes: BTreeMap::new(),
            reason: Some(reason.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotlightMetadataReader;

impl Default for SpotlightMetadataReader {
    fn default() -> Self {
        Self
    }
}

impl SpotlightMetadataReader {
    pub fn read_path(&self, path: impl AsRef<Path>) -> Result<SpotlightSnapshot> {
        Ok(self
            .read_paths([path.as_ref()])?
            .into_iter()
            .next()
            .expect("single-path Spotlight batch should always return one snapshot"))
    }

    pub fn read_paths<'a>(
        &self,
        paths: impl IntoIterator<Item = &'a Path>,
    ) -> Result<Vec<SpotlightSnapshot>> {
        let paths = paths.into_iter().map(Path::to_path_buf).collect::<Vec<_>>();
        let path_refs = paths.iter().map(PathBuf::as_path).collect::<Vec<_>>();
        let keys = SPOTLIGHT_FIELDS
            .iter()
            .map(|field| field.key())
            .collect::<Vec<_>>();
        read_spotlight_attributes_batch(&path_refs, &keys)
            .map(|snapshots| {
                paths
                    .into_iter()
                    .zip(snapshots)
                    .map(|(path, snapshot)| native_snapshot(path, snapshot))
                    .collect()
            })
            .map_err(|err| GfmError::Format(format!("failed to read Spotlight metadata: {err}")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotlightFieldDecision {
    PrimaryMatch,
    EnrichFromSpotlight,
    ConflictPrimaryWins,
    MissingFromSpotlight,
    SpotlightUnavailable,
}

impl SpotlightFieldDecision {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PrimaryMatch => "primary-match",
            Self::EnrichFromSpotlight => "enrich-from-spotlight",
            Self::ConflictPrimaryWins => "conflict-primary-wins",
            Self::MissingFromSpotlight => "missing-from-spotlight",
            Self::SpotlightUnavailable => "spotlight-unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotlightFieldReconciliation {
    pub field: SpotlightField,
    pub primary_values: Vec<String>,
    pub spotlight_values: Vec<String>,
    pub decision: SpotlightFieldDecision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotlightReconciliationReport {
    pub primary: FileRecord,
    pub snapshot: SpotlightSnapshot,
    pub fields: Vec<SpotlightFieldReconciliation>,
}

impl SpotlightReconciliationReport {
    pub fn reconcile(primary: FileRecord, snapshot: SpotlightSnapshot) -> Self {
        let fields = [
            SpotlightField::DisplayName,
            SpotlightField::Kind,
            SpotlightField::ContentType,
            SpotlightField::FinderComment,
            SpotlightField::UserTags,
            SpotlightField::Authors,
            SpotlightField::WhereFroms,
            SpotlightField::LastUsedDate,
        ]
        .into_iter()
        .map(|field| reconcile_field(field, &primary, &snapshot))
        .collect();

        Self {
            primary,
            snapshot,
            fields,
        }
    }

    pub fn enrichments(&self) -> usize {
        self.fields
            .iter()
            .filter(|field| field.decision == SpotlightFieldDecision::EnrichFromSpotlight)
            .count()
    }

    pub fn conflicts(&self) -> usize {
        self.fields
            .iter()
            .filter(|field| field.decision == SpotlightFieldDecision::ConflictPrimaryWins)
            .count()
    }

    pub fn as_tsv(&self) -> String {
        let reason = self
            .snapshot
            .reason
            .as_deref()
            .map(escape_field)
            .unwrap_or_else(|| "-".to_string());
        let mut lines = vec![format!(
            "spotlight-reconciliation\t{}\t{}:{}\tprimary=filesystem\tspotlight={}\tfields={}\tenrichments={}\tconflicts={}\treason={}",
            escape_path_field(&self.primary.path),
            self.primary.id.volume.0,
            self.primary.id.node,
            self.snapshot.status.as_str(),
            self.fields.len(),
            self.enrichments(),
            self.conflicts(),
            reason,
        )];
        lines.extend(self.fields.iter().map(|field| {
            format!(
                "field\t{}\tprimary={}\tspotlight={}\tdecision={}",
                field.field.as_str(),
                values_tsv(&field.primary_values),
                values_tsv(&field.spotlight_values),
                field.decision.as_str()
            )
        }));
        lines.join("\n")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotlightIndexHealth {
    Healthy,
    Degraded,
    Unavailable,
}

impl SpotlightIndexHealth {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotlightIngestionAction {
    Publish,
    QuarantineStale,
    DeferVolumeThrottle,
    SkipUnavailable,
}

impl SpotlightIngestionAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Publish => "publish",
            Self::QuarantineStale => "quarantine-stale",
            Self::DeferVolumeThrottle => "defer-volume-throttle",
            Self::SkipUnavailable => "skip-unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotlightIngestionPolicy {
    pub max_unavailable_fraction_bps: u32,
    pub max_missing_fraction_bps: u32,
    pub max_records_per_volume: usize,
    pub stale_conflict_fields: Vec<SpotlightField>,
}

impl Default for SpotlightIngestionPolicy {
    fn default() -> Self {
        Self {
            max_unavailable_fraction_bps: 1_000,
            max_missing_fraction_bps: 2_500,
            max_records_per_volume: 512,
            stale_conflict_fields: vec![SpotlightField::DisplayName],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotlightIngestionDecision {
    pub id: FileId,
    pub volume: VolumeId,
    pub path: PathBuf,
    pub action: SpotlightIngestionAction,
    pub reason: String,
    pub publishable_attributes: BTreeMap<SpotlightField, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotlightIngestionPlan {
    pub health: SpotlightIndexHealth,
    pub total: usize,
    pub publishable: usize,
    pub quarantined: usize,
    pub deferred: usize,
    pub unavailable: usize,
    pub decisions: Vec<SpotlightIngestionDecision>,
}

impl SpotlightIngestionPlan {
    pub fn from_records(
        records: &[FileRecord],
        snapshots: &[SpotlightSnapshot],
        policy: &SpotlightIngestionPolicy,
    ) -> Self {
        let total = records.len().min(snapshots.len());
        let health = spotlight_index_health(&snapshots[..total], policy);
        let mut volume_counts = BTreeMap::<VolumeId, usize>::new();
        let mut decisions = Vec::with_capacity(total);
        let mut publishable = 0;
        let mut quarantined = 0;
        let mut deferred = 0;
        let mut unavailable = 0;

        for (record, snapshot) in records.iter().zip(snapshots).take(total) {
            let decision = if health == SpotlightIndexHealth::Unavailable
                || snapshot.status != SpotlightStatus::Available
            {
                unavailable += 1;
                ingestion_decision(
                    record,
                    SpotlightIngestionAction::SkipUnavailable,
                    snapshot
                        .reason
                        .clone()
                        .unwrap_or_else(|| "Spotlight snapshot is unavailable".to_string()),
                    BTreeMap::new(),
                )
            } else if stale_snapshot(record, snapshot, &policy.stale_conflict_fields) {
                quarantined += 1;
                ingestion_decision(
                    record,
                    SpotlightIngestionAction::QuarantineStale,
                    "Spotlight snapshot conflicts with primary filesystem identity".to_string(),
                    BTreeMap::new(),
                )
            } else {
                let count = volume_counts.entry(record.id.volume).or_default();
                if *count >= policy.max_records_per_volume {
                    deferred += 1;
                    ingestion_decision(
                        record,
                        SpotlightIngestionAction::DeferVolumeThrottle,
                        format!(
                            "volume {} exceeded Spotlight metadata budget of {} records",
                            record.id.volume.0, policy.max_records_per_volume
                        ),
                        BTreeMap::new(),
                    )
                } else {
                    *count += 1;
                    publishable += 1;
                    ingestion_decision(
                        record,
                        SpotlightIngestionAction::Publish,
                        "Spotlight metadata accepted for secondary index publication".to_string(),
                        snapshot.attributes.clone(),
                    )
                }
            };
            decisions.push(decision);
        }

        Self {
            health,
            total,
            publishable,
            quarantined,
            deferred,
            unavailable,
            decisions,
        }
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = vec![format!(
            "spotlight-ingestion-plan\thealth={}\ttotal={}\tpublishable={}\tquarantined={}\tdeferred={}\tunavailable={}",
            self.health.as_str(),
            self.total,
            self.publishable,
            self.quarantined,
            self.deferred,
            self.unavailable,
        )];
        lines.extend(self.decisions.iter().map(|decision| {
            format!(
                "decision\t{}:{}\t{}\taction={}\treason={}\tattributes={}",
                decision.id.volume.0,
                decision.id.node,
                escape_path_field(&decision.path),
                decision.action.as_str(),
                escape_field(&decision.reason),
                decision.publishable_attributes.len(),
            )
        }));
        lines.join("\n")
    }

    pub fn secondary_metadata_records(&self) -> Vec<SecondaryMetadataRecord> {
        self.decisions
            .iter()
            .filter(|decision| decision.action == SpotlightIngestionAction::Publish)
            .filter_map(|decision| {
                let tags = spotlight_secondary_tags(&decision.publishable_attributes);
                let comments = spotlight_secondary_values(
                    &decision.publishable_attributes,
                    &[
                        SpotlightField::FinderComment,
                        SpotlightField::Kind,
                        SpotlightField::ContentType,
                        SpotlightField::Authors,
                        SpotlightField::WhereFroms,
                    ],
                );
                (!tags.is_empty() || !comments.is_empty()).then_some(SecondaryMetadataRecord {
                    id: decision.id,
                    tags,
                    comments,
                })
            })
            .collect()
    }
}

pub fn parse_spotlight_fixture(path: impl Into<PathBuf>, text: &str) -> Result<SpotlightSnapshot> {
    let path = path.into();
    let mut attributes: BTreeMap<SpotlightField, Vec<String>> = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line.split_once('\t').ok_or_else(|| {
            GfmError::Format(format!(
                "invalid spotlight fixture line {}: expected key<TAB>value",
                index + 1
            ))
        })?;
        let field = SpotlightField::from_key(key).ok_or_else(|| {
            GfmError::Format(format!("unsupported spotlight fixture key `{key}`"))
        })?;
        attributes.insert(field, split_fixture_values(value));
    }
    Ok(SpotlightSnapshot::available(path, attributes))
}

fn spotlight_index_health(
    snapshots: &[SpotlightSnapshot],
    policy: &SpotlightIngestionPolicy,
) -> SpotlightIndexHealth {
    if snapshots.is_empty() {
        return SpotlightIndexHealth::Healthy;
    }
    let unavailable = snapshots
        .iter()
        .filter(|snapshot| snapshot.status == SpotlightStatus::Unavailable)
        .count();
    let missing = snapshots
        .iter()
        .filter(|snapshot| snapshot.status == SpotlightStatus::Missing)
        .count();
    if unavailable == snapshots.len() {
        return SpotlightIndexHealth::Unavailable;
    }
    if fraction_bps(unavailable, snapshots.len()) > policy.max_unavailable_fraction_bps
        || fraction_bps(missing, snapshots.len()) > policy.max_missing_fraction_bps
    {
        SpotlightIndexHealth::Degraded
    } else {
        SpotlightIndexHealth::Healthy
    }
}

fn fraction_bps(count: usize, total: usize) -> u32 {
    ((count.saturating_mul(10_000)) / total.max(1)) as u32
}

fn stale_snapshot(
    record: &FileRecord,
    snapshot: &SpotlightSnapshot,
    stale_fields: &[SpotlightField],
) -> bool {
    if snapshot.path != record.path {
        return true;
    }
    let report = SpotlightReconciliationReport::reconcile(record.clone(), snapshot.clone());
    report.fields.iter().any(|field| {
        stale_fields.contains(&field.field)
            && field.decision == SpotlightFieldDecision::ConflictPrimaryWins
    })
}

fn ingestion_decision(
    record: &FileRecord,
    action: SpotlightIngestionAction,
    reason: String,
    publishable_attributes: BTreeMap<SpotlightField, Vec<String>>,
) -> SpotlightIngestionDecision {
    SpotlightIngestionDecision {
        id: record.id,
        volume: record.id.volume,
        path: record.path.clone(),
        action,
        reason,
        publishable_attributes,
    }
}

fn spotlight_secondary_values(
    attributes: &BTreeMap<SpotlightField, Vec<String>>,
    fields: &[SpotlightField],
) -> Vec<String> {
    let mut values = BTreeSet::new();
    for field in fields {
        if let Some(field_values) = attributes.get(field) {
            for value in field_values {
                let value = value.trim();
                if !value.is_empty() {
                    values.insert(value.to_string());
                }
            }
        }
    }
    values.into_iter().collect()
}

fn spotlight_secondary_tags(attributes: &BTreeMap<SpotlightField, Vec<String>>) -> Vec<String> {
    let mut tags = BTreeSet::new();
    if let Some(values) = attributes.get(&SpotlightField::UserTags) {
        for value in values {
            for tag in value.split(',') {
                let tag = tag.trim();
                if !tag.is_empty() {
                    tags.insert(tag.to_string());
                }
            }
        }
    }
    tags.into_iter().collect()
}

fn native_snapshot(
    path: PathBuf,
    snapshot: gfm_mac_sys::NativeSpotlightSnapshot,
) -> SpotlightSnapshot {
    match snapshot.status {
        NativeSpotlightStatus::Available => SpotlightSnapshot::available(
            path,
            snapshot
                .attributes
                .into_iter()
                .filter_map(|(key, values)| {
                    SpotlightField::from_key(&key).map(|field| (field, values))
                })
                .collect(),
        ),
        NativeSpotlightStatus::Missing => SpotlightSnapshot::missing(
            path,
            snapshot
                .reason
                .unwrap_or_else(|| "Spotlight metadata item is missing".to_string()),
        ),
        NativeSpotlightStatus::Unavailable => SpotlightSnapshot::unavailable(
            path,
            snapshot
                .reason
                .unwrap_or_else(|| "Spotlight metadata is unavailable".to_string()),
        ),
    }
}

fn reconcile_field(
    field: SpotlightField,
    primary: &FileRecord,
    snapshot: &SpotlightSnapshot,
) -> SpotlightFieldReconciliation {
    let primary_values = primary_values(field, primary);
    let spotlight_values = snapshot.attributes.get(&field).cloned().unwrap_or_default();
    let decision = if snapshot.status != SpotlightStatus::Available {
        SpotlightFieldDecision::SpotlightUnavailable
    } else if spotlight_values.is_empty() {
        SpotlightFieldDecision::MissingFromSpotlight
    } else if primary_values.is_empty() {
        SpotlightFieldDecision::EnrichFromSpotlight
    } else if normalized_set(&primary_values) == normalized_set(&spotlight_values) {
        SpotlightFieldDecision::PrimaryMatch
    } else if field == SpotlightField::Kind || field == SpotlightField::ContentType {
        SpotlightFieldDecision::EnrichFromSpotlight
    } else {
        SpotlightFieldDecision::ConflictPrimaryWins
    };

    SpotlightFieldReconciliation {
        field,
        primary_values,
        spotlight_values,
        decision,
    }
}

fn primary_values(field: SpotlightField, primary: &FileRecord) -> Vec<String> {
    match field {
        SpotlightField::DisplayName => vec![primary.name.clone()],
        SpotlightField::Kind => vec![file_kind(primary.kind).to_string()],
        SpotlightField::ContentType => primary
            .extension()
            .map(|extension| format!("extension:{extension}"))
            .into_iter()
            .collect(),
        SpotlightField::UserTags => primary.tags.clone(),
        SpotlightField::FinderComment
        | SpotlightField::Authors
        | SpotlightField::WhereFroms
        | SpotlightField::LastUsedDate => Vec::new(),
    }
}

fn file_kind(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Directory => "directory",
        FileKind::File => "file",
        FileKind::Symlink => "symlink",
        FileKind::Other => "other",
    }
}

fn split_fixture_values(value: &str) -> Vec<String> {
    value
        .split('|')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn normalized_set(values: &[String]) -> BTreeSet<String> {
    values
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect()
}

fn values_tsv(values: &[String]) -> String {
    if values.is_empty() {
        "-".to_string()
    } else {
        escape_field(&values.join("|"))
    }
}

fn escape_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn escape_path_field(path: &Path) -> String {
    escape_field(&path.to_string_lossy())
}

#[cfg(test)]
mod tests;
