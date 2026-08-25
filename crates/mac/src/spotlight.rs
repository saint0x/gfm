use gfm_types::{FileKind, FileRecord, GfmError, Result};
use plist::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

const SPOTLIGHT_MAX_PLIST_BYTES: usize = 512 * 1024;

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
pub struct SpotlightMetadataReader {
    max_plist_bytes: usize,
}

impl Default for SpotlightMetadataReader {
    fn default() -> Self {
        Self {
            max_plist_bytes: SPOTLIGHT_MAX_PLIST_BYTES,
        }
    }
}

impl SpotlightMetadataReader {
    pub fn read_path(&self, path: impl AsRef<Path>) -> Result<SpotlightSnapshot> {
        let path = path.as_ref().to_path_buf();
        let output = Command::new("mdls")
            .args(["-plist", "-"])
            .arg(&path)
            .output()
            .map_err(|err| GfmError::Format(format!("failed to run mdls: {err}")))?;

        if !output.status.success() {
            let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if reason.contains("could not find") {
                return Ok(SpotlightSnapshot::missing(path, reason));
            }
            return Ok(SpotlightSnapshot::unavailable(path, reason));
        }
        if output.stdout.len() > self.max_plist_bytes {
            return Ok(SpotlightSnapshot::unavailable(
                path,
                format!(
                    "mdls plist output exceeded {} byte budget",
                    self.max_plist_bytes
                ),
            ));
        }
        parse_mdls_plist(&path, &output.stdout)
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
            self.primary.path.display(),
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

fn parse_mdls_plist(path: &Path, bytes: &[u8]) -> Result<SpotlightSnapshot> {
    let value = Value::from_reader_xml(bytes)
        .map_err(|err| GfmError::Format(format!("failed to parse mdls plist: {err}")))?;
    let dict = value
        .as_dictionary()
        .ok_or_else(|| GfmError::Format("mdls plist root was not a dictionary".to_string()))?;
    let mut attributes = BTreeMap::new();
    for (key, value) in dict {
        let Some(field) = SpotlightField::from_key(key) else {
            continue;
        };
        let values = plist_values(value);
        if !values.is_empty() {
            attributes.insert(field, values);
        }
    }
    Ok(SpotlightSnapshot::available(path, attributes))
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

fn plist_values(value: &Value) -> Vec<String> {
    match value {
        Value::String(value) => (!value.is_empty())
            .then(|| value.clone())
            .into_iter()
            .collect(),
        Value::Array(values) => values.iter().flat_map(plist_values).collect(),
        Value::Date(value) => vec![value.to_xml_format()],
        Value::Boolean(value) => vec![value.to_string()],
        Value::Integer(value) => vec![value.to_string()],
        Value::Real(value) => vec![value.to_string()],
        _ => Vec::new(),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_types::{FileId, VolumeId};

    #[test]
    fn reconciles_spotlight_enrichment_without_primary_dependency() {
        let primary = record("Report.md");
        let snapshot = parse_spotlight_fixture(
            &primary.path,
            "kMDItemDisplayName\tReport.md\nkMDItemKind\tMarkdown Document\nkMDItemFinderComment\tclient handoff\nkMDItemUserTags\tImportant|Client\n",
        )
        .unwrap();

        let report = SpotlightReconciliationReport::reconcile(primary, snapshot);

        assert_eq!(report.snapshot.status, SpotlightStatus::Available);
        assert_eq!(report.enrichments(), 2);
        assert_eq!(report.conflicts(), 0);
        assert!(report.as_tsv().contains(
            "field\tfinder-comment\tprimary=-\tspotlight=client handoff\tdecision=enrich-from-spotlight"
        ));
    }

    #[test]
    fn primary_display_name_wins_spotlight_conflict() {
        let primary = record("Primary.md");
        let snapshot =
            parse_spotlight_fixture(&primary.path, "kMDItemDisplayName\tStale.md\n").unwrap();

        let report = SpotlightReconciliationReport::reconcile(primary, snapshot);

        assert_eq!(report.conflicts(), 1);
        assert!(report.as_tsv().contains(
            "field\tdisplay-name\tprimary=Primary.md\tspotlight=Stale.md\tdecision=conflict-primary-wins"
        ));
    }

    #[test]
    fn unavailable_spotlight_never_blocks_primary_record() {
        let primary = record("Local.txt");
        let snapshot = SpotlightSnapshot::missing(&primary.path, "mdls could not find Local.txt");

        let report = SpotlightReconciliationReport::reconcile(primary, snapshot);

        assert!(report
            .fields
            .iter()
            .all(|field| field.decision == SpotlightFieldDecision::SpotlightUnavailable));
        assert!(report.as_tsv().starts_with(
            "spotlight-reconciliation\t/tmp/Local.txt\t1:9\tprimary=filesystem\tspotlight=missing"
        ));
    }

    fn record(name: &str) -> FileRecord {
        FileRecord {
            id: FileId::new(VolumeId(1), 9),
            parent: None,
            path: PathBuf::from("/tmp").join(name),
            name: name.to_string(),
            kind: FileKind::File,
            len: 42,
            mode: 0,
            owner: 0,
            group: 0,
            xattrs_digest: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: vec!["Important".to_string(), "Client".to_string()],
            finder_comment: None,
        }
    }
}
