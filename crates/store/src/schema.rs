use crate::{
    read_content_postings, read_metadata_postings, read_records, write_content_postings,
    write_metadata_postings, write_records, ContentArchive, ContentArchiveManifest,
    MmapContentArchive, MmapDictionary, MmapFuzzyArchive, MmapMetadataArchive, MmapPrefixArchive,
    MmapRecordArchive, MmapRecordColumns, MmapSubstringArchive,
};
use gfm_types::{GfmError, Result};
use std::fmt;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const RECORD_CURRENT: &str = "gfm-store-v3";
const RECORD_LEGACY: &[&str] = &["gfm-store-v1", "gfm-store-v2"];
const COLUMNS_CURRENT: &[u8] = b"gfm-record-columns-v2\n";
const COLUMNS_LEGACY: &[&[u8]] = &[b"gfm-record-columns-v1\n"];
const METADATA_CURRENT: &[u8] = b"gfm-metadata-v3\n";
const METADATA_LEGACY: &[&[u8]] = &[b"gfm-metadata-v1\n", b"gfm-metadata-v2\n"];
const PREFIX_CURRENT: &[u8] = b"gfm-prefix-v1\n";
const SUBSTRING_CURRENT: &[u8] = b"gfm-substring-v1\n";
const FUZZY_CURRENT: &[u8] = b"gfm-fuzzy-v1\n";
const DICTIONARY_CURRENT: &[u8] = b"gfm-dictionary-v1\n";
const CONTENT_CURRENT: &[u8] = b"gfm-content-v5\n";
const CONTENT_LEGACY: &[&[u8]] = &[
    b"gfm-content-v1\n",
    b"gfm-content-v2\n",
    b"gfm-content-v3\n",
    b"gfm-content-v4\n",
];
const CONTENT_MANIFEST_CURRENT: &str = "gfm-content-manifest-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveSchemaKind {
    Records,
    Columns,
    Metadata,
    Prefixes,
    Substrings,
    Fuzzy,
    Dictionary,
    Content,
    ContentManifest,
}

impl ArchiveSchemaKind {
    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "records" => Some(Self::Records),
            "columns" => Some(Self::Columns),
            "metadata" => Some(Self::Metadata),
            "prefixes" | "prefix" => Some(Self::Prefixes),
            "substrings" | "substring" => Some(Self::Substrings),
            "fuzzy" => Some(Self::Fuzzy),
            "dictionary" => Some(Self::Dictionary),
            "content" => Some(Self::Content),
            "content-manifest" => Some(Self::ContentManifest),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Records => "records",
            Self::Columns => "columns",
            Self::Metadata => "metadata",
            Self::Prefixes => "prefixes",
            Self::Substrings => "substrings",
            Self::Fuzzy => "fuzzy",
            Self::Dictionary => "dictionary",
            Self::Content => "content",
            Self::ContentManifest => "content-manifest",
        }
    }

    pub const fn current_schema(self) -> &'static str {
        match self {
            Self::Records => RECORD_CURRENT,
            Self::Columns => "gfm-record-columns-v2",
            Self::Metadata => "gfm-metadata-v3",
            Self::Prefixes => "gfm-prefix-v1",
            Self::Substrings => "gfm-substring-v1",
            Self::Fuzzy => "gfm-fuzzy-v1",
            Self::Dictionary => "gfm-dictionary-v1",
            Self::Content => "gfm-content-v5",
            Self::ContentManifest => CONTENT_MANIFEST_CURRENT,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveSchemaStatus {
    Current,
    Legacy,
    Unsupported,
    Missing,
    Unreadable,
}

impl ArchiveSchemaStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Legacy => "legacy",
            Self::Unsupported => "unsupported",
            Self::Missing => "missing",
            Self::Unreadable => "unreadable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveSchemaReport {
    pub kind: ArchiveSchemaKind,
    pub path: PathBuf,
    pub status: ArchiveSchemaStatus,
    pub schema: Option<String>,
    pub current_schema: &'static str,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordArchiveMigrationAction {
    Ready,
    Migrate,
    CannotMigrate,
}

impl RecordArchiveMigrationAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Migrate => "migrate",
            Self::CannotMigrate => "cannot-migrate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordArchiveMigrationPlan {
    pub action: RecordArchiveMigrationAction,
    pub before: ArchiveSchemaReport,
    pub detail: Option<String>,
}

impl RecordArchiveMigrationPlan {
    pub fn as_tsv(&self) -> String {
        format!(
            "record-archive-migration-plan\taction={}\tstatus={}\tschema={}\tcurrent={}\tpath={}\tdetail={}",
            self.action.as_str(),
            self.before.status.as_str(),
            self.before.schema.as_deref().unwrap_or("-"),
            self.before.current_schema,
            escape_field(&self.before.path.display().to_string()),
            self.detail.as_deref().map(escape_field).unwrap_or("-".to_string())
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordArchiveMigration {
    pub before: RecordArchiveMigrationPlan,
    pub after: ArchiveSchemaReport,
    pub migrated_records: usize,
    pub backup_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentArchiveMigrationAction {
    Ready,
    Migrate,
    CannotMigrate,
}

impl ContentArchiveMigrationAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Migrate => "migrate",
            Self::CannotMigrate => "cannot-migrate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentArchiveMigrationPlan {
    pub action: ContentArchiveMigrationAction,
    pub before: ArchiveSchemaReport,
    pub detail: Option<String>,
}

impl ContentArchiveMigrationPlan {
    pub fn as_tsv(&self) -> String {
        format!(
            "content-archive-migration-plan\taction={}\tstatus={}\tschema={}\tcurrent={}\tpath={}\tdetail={}",
            self.action.as_str(),
            self.before.status.as_str(),
            self.before.schema.as_deref().unwrap_or("-"),
            self.before.current_schema,
            escape_field(&self.before.path.display().to_string()),
            self.detail.as_deref().map(escape_field).unwrap_or("-".to_string())
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentArchiveMigration {
    pub before: ContentArchiveMigrationPlan,
    pub after: ArchiveSchemaReport,
    pub migrated_postings: usize,
    pub backup_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataArchiveMigrationAction {
    Ready,
    Migrate,
    CannotMigrate,
}

impl MetadataArchiveMigrationAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Migrate => "migrate",
            Self::CannotMigrate => "cannot-migrate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataArchiveMigrationPlan {
    pub action: MetadataArchiveMigrationAction,
    pub before: ArchiveSchemaReport,
    pub detail: Option<String>,
}

impl MetadataArchiveMigrationPlan {
    pub fn as_tsv(&self) -> String {
        format!(
            "metadata-archive-migration-plan\taction={}\tstatus={}\tschema={}\tcurrent={}\tpath={}\tdetail={}",
            self.action.as_str(),
            self.before.status.as_str(),
            self.before.schema.as_deref().unwrap_or("-"),
            self.before.current_schema,
            escape_field(&self.before.path.display().to_string()),
            self.detail.as_deref().map(escape_field).unwrap_or("-".to_string())
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataArchiveMigration {
    pub before: MetadataArchiveMigrationPlan,
    pub after: ArchiveSchemaReport,
    pub migrated_postings: usize,
    pub backup_path: Option<PathBuf>,
}

impl MetadataArchiveMigration {
    pub fn as_tsv(&self) -> String {
        format!(
            "metadata-archive-migration\tmigrated-postings={}\tbefore-status={}\tafter-status={}\tbackup={}\tpath={}",
            self.migrated_postings,
            self.before.before.status.as_str(),
            self.after.status.as_str(),
            self.backup_path
                .as_ref()
                .map(|path| escape_field(&path.display().to_string()))
                .unwrap_or("-".to_string()),
            escape_field(&self.after.path.display().to_string())
        )
    }
}

impl ContentArchiveMigration {
    pub fn as_tsv(&self) -> String {
        format!(
            "content-archive-migration\tmigrated-postings={}\tbefore-status={}\tafter-status={}\tbackup={}\tpath={}",
            self.migrated_postings,
            self.before.before.status.as_str(),
            self.after.status.as_str(),
            self.backup_path
                .as_ref()
                .map(|path| escape_field(&path.display().to_string()))
                .unwrap_or("-".to_string()),
            escape_field(&self.after.path.display().to_string())
        )
    }
}

impl RecordArchiveMigration {
    pub fn as_tsv(&self) -> String {
        format!(
            "record-archive-migration\tmigrated-records={}\tbefore-status={}\tafter-status={}\tbackup={}\tpath={}",
            self.migrated_records,
            self.before.before.status.as_str(),
            self.after.status.as_str(),
            self.backup_path
                .as_ref()
                .map(|path| escape_field(&path.display().to_string()))
                .unwrap_or("-".to_string()),
            escape_field(&self.after.path.display().to_string())
        )
    }
}

impl ArchiveSchemaReport {
    pub fn as_tsv(&self) -> String {
        format!(
            "archive-schema\tkind={}\tstatus={}\tschema={}\tcurrent={}\tpath={}\tdetail={}",
            self.kind.as_str(),
            self.status.as_str(),
            self.schema.as_deref().unwrap_or("-"),
            self.current_schema,
            escape_field(&self.path.display().to_string()),
            self.detail
                .as_deref()
                .map(escape_field)
                .unwrap_or("-".to_string())
        )
    }
}

pub fn inspect_archive_schema(
    kind: ArchiveSchemaKind,
    path: impl AsRef<Path>,
) -> ArchiveSchemaReport {
    let path = path.as_ref();
    match inspect_archive_schema_result(kind, path) {
        Ok(report) => report,
        Err(detail) => ArchiveSchemaReport {
            kind,
            path: path.to_path_buf(),
            status: ArchiveSchemaStatus::Unreadable,
            schema: None,
            current_schema: kind.current_schema(),
            detail: Some(detail.to_string()),
        },
    }
}

pub fn plan_record_archive_migration(path: impl AsRef<Path>) -> RecordArchiveMigrationPlan {
    let before = inspect_archive_schema(ArchiveSchemaKind::Records, path);
    let (action, detail) = match before.status {
        ArchiveSchemaStatus::Current => (
            RecordArchiveMigrationAction::Ready,
            Some("record archive is already current".to_string()),
        ),
        ArchiveSchemaStatus::Legacy => (
            RecordArchiveMigrationAction::Migrate,
            Some("legacy record archive can be rewritten as current gfm-store-v3".to_string()),
        ),
        ArchiveSchemaStatus::Missing => (
            RecordArchiveMigrationAction::CannotMigrate,
            Some("missing record archive must be rebuilt from filesystem scan".to_string()),
        ),
        ArchiveSchemaStatus::Unsupported => (
            RecordArchiveMigrationAction::CannotMigrate,
            Some("unsupported record archive schema cannot be migrated".to_string()),
        ),
        ArchiveSchemaStatus::Unreadable => (
            RecordArchiveMigrationAction::CannotMigrate,
            Some("unreadable record archive must be quarantined and rebuilt".to_string()),
        ),
    };
    RecordArchiveMigrationPlan {
        action,
        before,
        detail,
    }
}

pub fn migrate_record_archive(
    path: impl AsRef<Path>,
    backup_dir: impl AsRef<Path>,
) -> Result<RecordArchiveMigration> {
    let path = path.as_ref();
    let backup_dir = backup_dir.as_ref();
    let before = plan_record_archive_migration(path);
    match before.action {
        RecordArchiveMigrationAction::Ready => {
            return Ok(RecordArchiveMigration {
                after: before.before.clone(),
                before,
                migrated_records: 0,
                backup_path: None,
            });
        }
        RecordArchiveMigrationAction::CannotMigrate => {
            return Err(GfmError::Format(format!(
                "{} cannot be migrated: {}",
                path.display(),
                before
                    .detail
                    .as_deref()
                    .unwrap_or("unsupported migration state")
            )));
        }
        RecordArchiveMigrationAction::Migrate => {}
    }

    let records = read_records(path)?;
    let backup_path = backup_archive(path, backup_dir, "legacy")?;
    write_records(path, &records)?;
    let after = inspect_archive_schema(ArchiveSchemaKind::Records, path);
    if after.status != ArchiveSchemaStatus::Current {
        return Err(GfmError::Format(format!(
            "{} migration produced {} instead of current schema",
            path.display(),
            after.status.as_str()
        )));
    }
    Ok(RecordArchiveMigration {
        before,
        after,
        migrated_records: records.len(),
        backup_path: Some(backup_path),
    })
}

pub fn plan_content_archive_migration(path: impl AsRef<Path>) -> ContentArchiveMigrationPlan {
    let before = inspect_archive_schema(ArchiveSchemaKind::Content, path);
    let (action, detail) = match before.status {
        ArchiveSchemaStatus::Current => (
            ContentArchiveMigrationAction::Ready,
            Some("content archive is already current".to_string()),
        ),
        ArchiveSchemaStatus::Legacy => (
            ContentArchiveMigrationAction::Migrate,
            Some("legacy content archive can be rewritten as current gfm-content-v5".to_string()),
        ),
        ArchiveSchemaStatus::Missing => (
            ContentArchiveMigrationAction::CannotMigrate,
            Some("missing content archive must be rebuilt from extraction segments".to_string()),
        ),
        ArchiveSchemaStatus::Unsupported => (
            ContentArchiveMigrationAction::CannotMigrate,
            Some("unsupported content archive schema cannot be migrated".to_string()),
        ),
        ArchiveSchemaStatus::Unreadable => (
            ContentArchiveMigrationAction::CannotMigrate,
            Some("unreadable content archive must be quarantined and rebuilt".to_string()),
        ),
    };
    ContentArchiveMigrationPlan {
        action,
        before,
        detail,
    }
}

pub fn migrate_content_archive(
    path: impl AsRef<Path>,
    backup_dir: impl AsRef<Path>,
) -> Result<ContentArchiveMigration> {
    let path = path.as_ref();
    let backup_dir = backup_dir.as_ref();
    let before = plan_content_archive_migration(path);
    match before.action {
        ContentArchiveMigrationAction::Ready => {
            return Ok(ContentArchiveMigration {
                after: before.before.clone(),
                before,
                migrated_postings: 0,
                backup_path: None,
            });
        }
        ContentArchiveMigrationAction::CannotMigrate => {
            return Err(GfmError::Format(format!(
                "{} cannot be migrated: {}",
                path.display(),
                before
                    .detail
                    .as_deref()
                    .unwrap_or("unsupported migration state")
            )));
        }
        ContentArchiveMigrationAction::Migrate => {}
    }

    let postings = read_content_postings(path)?;
    let backup_path = backup_archive(path, backup_dir, "legacy")?;
    write_content_postings(path, &postings)?;
    let after = inspect_archive_schema(ArchiveSchemaKind::Content, path);
    if after.status != ArchiveSchemaStatus::Current {
        return Err(GfmError::Format(format!(
            "{} migration produced {} instead of current schema",
            path.display(),
            after.status.as_str()
        )));
    }
    Ok(ContentArchiveMigration {
        before,
        after,
        migrated_postings: postings.len(),
        backup_path: Some(backup_path),
    })
}

pub fn plan_metadata_archive_migration(path: impl AsRef<Path>) -> MetadataArchiveMigrationPlan {
    let before = inspect_archive_schema(ArchiveSchemaKind::Metadata, path);
    let (action, detail) = match before.status {
        ArchiveSchemaStatus::Current => (
            MetadataArchiveMigrationAction::Ready,
            Some("metadata archive is already current".to_string()),
        ),
        ArchiveSchemaStatus::Legacy => (
            MetadataArchiveMigrationAction::Migrate,
            Some("legacy metadata archive can be rewritten as current gfm-metadata-v3".to_string()),
        ),
        ArchiveSchemaStatus::Missing => (
            MetadataArchiveMigrationAction::CannotMigrate,
            Some("missing metadata archive must be rebuilt from durable records".to_string()),
        ),
        ArchiveSchemaStatus::Unsupported => (
            MetadataArchiveMigrationAction::CannotMigrate,
            Some("unsupported metadata archive schema cannot be migrated".to_string()),
        ),
        ArchiveSchemaStatus::Unreadable => (
            MetadataArchiveMigrationAction::CannotMigrate,
            Some("unreadable metadata archive must be quarantined and rebuilt".to_string()),
        ),
    };
    MetadataArchiveMigrationPlan {
        action,
        before,
        detail,
    }
}

pub fn migrate_metadata_archive(
    path: impl AsRef<Path>,
    backup_dir: impl AsRef<Path>,
) -> Result<MetadataArchiveMigration> {
    let path = path.as_ref();
    let backup_dir = backup_dir.as_ref();
    let before = plan_metadata_archive_migration(path);
    match before.action {
        MetadataArchiveMigrationAction::Ready => {
            return Ok(MetadataArchiveMigration {
                after: before.before.clone(),
                before,
                migrated_postings: 0,
                backup_path: None,
            });
        }
        MetadataArchiveMigrationAction::CannotMigrate => {
            return Err(GfmError::Format(format!(
                "{} cannot be migrated: {}",
                path.display(),
                before
                    .detail
                    .as_deref()
                    .unwrap_or("unsupported migration state")
            )));
        }
        MetadataArchiveMigrationAction::Migrate => {}
    }

    let postings = read_metadata_postings(path)?;
    let backup_path = backup_archive(path, backup_dir, "legacy")?;
    write_metadata_postings(path, &postings)?;
    let after = inspect_archive_schema(ArchiveSchemaKind::Metadata, path);
    if after.status != ArchiveSchemaStatus::Current {
        return Err(GfmError::Format(format!(
            "{} migration produced {} instead of current schema",
            path.display(),
            after.status.as_str()
        )));
    }
    Ok(MetadataArchiveMigration {
        before,
        after,
        migrated_postings: postings.len(),
        backup_path: Some(backup_path),
    })
}

fn inspect_archive_schema_result(
    kind: ArchiveSchemaKind,
    path: &Path,
) -> Result<ArchiveSchemaReport> {
    if !path.exists() {
        return Ok(report(kind, path, ArchiveSchemaStatus::Missing, None, None));
    }
    match kind {
        ArchiveSchemaKind::Records => {
            inspect_line_schema(kind, path, &[RECORD_CURRENT], RECORD_LEGACY, || {
                MmapRecordArchive::open(path).map(|_| ())
            })
        }
        ArchiveSchemaKind::Columns => {
            inspect_magic_schema(kind, path, &[COLUMNS_CURRENT], COLUMNS_LEGACY, || {
                MmapRecordColumns::open(path).map(|_| ())
            })
        }
        ArchiveSchemaKind::Metadata => {
            inspect_magic_schema(kind, path, &[METADATA_CURRENT], METADATA_LEGACY, || {
                MmapMetadataArchive::open(path).map(|_| ())
            })
        }
        ArchiveSchemaKind::Prefixes => {
            inspect_magic_schema(kind, path, &[PREFIX_CURRENT], &[], || {
                MmapPrefixArchive::open(path).map(|_| ())
            })
        }
        ArchiveSchemaKind::Substrings => {
            inspect_magic_schema(kind, path, &[SUBSTRING_CURRENT], &[], || {
                MmapSubstringArchive::open(path).map(|_| ())
            })
        }
        ArchiveSchemaKind::Fuzzy => inspect_magic_schema(kind, path, &[FUZZY_CURRENT], &[], || {
            MmapFuzzyArchive::open(path).map(|_| ())
        }),
        ArchiveSchemaKind::Dictionary => {
            inspect_magic_schema(kind, path, &[DICTIONARY_CURRENT], &[], || {
                MmapDictionary::open(path).map(|_| ())
            })
        }
        ArchiveSchemaKind::Content => inspect_content_schema(kind, path),
        ArchiveSchemaKind::ContentManifest => {
            inspect_line_schema(kind, path, &[CONTENT_MANIFEST_CURRENT], &[], || {
                ContentArchiveManifest::read(path).map(|_| ())
            })
        }
    }
}

fn inspect_content_schema(kind: ArchiveSchemaKind, path: &Path) -> Result<ArchiveSchemaReport> {
    let max_len = CONTENT_LEGACY
        .iter()
        .chain([CONTENT_CURRENT].iter())
        .map(|magic| magic.len())
        .max()
        .unwrap_or(0);
    let mut bytes = vec![0; max_len];
    let mut file = File::open(path).map_err(|err| gfm_types::GfmError::io(path, err))?;
    let len = file
        .read(&mut bytes)
        .map_err(|err| gfm_types::GfmError::io(path, err))?;
    bytes.truncate(len);

    if let Some(schema) = matching_magic(&bytes, &[CONTENT_CURRENT]) {
        return validated_report(
            kind,
            path,
            ArchiveSchemaStatus::Current,
            Some(schema),
            || MmapContentArchive::open(path).map(|_| ()),
        );
    }
    if let Some(schema) = matching_magic(&bytes, CONTENT_LEGACY) {
        return validated_report(
            kind,
            path,
            ArchiveSchemaStatus::Legacy,
            Some(schema),
            || ContentArchive::open(path).map(|_| ()),
        );
    }
    Ok(report(
        kind,
        path,
        ArchiveSchemaStatus::Unsupported,
        None,
        Some("unsupported archive header".to_string()),
    ))
}

fn inspect_line_schema(
    kind: ArchiveSchemaKind,
    path: &Path,
    current: &[&str],
    legacy: &[&str],
    validate: impl FnOnce() -> Result<()>,
) -> Result<ArchiveSchemaReport> {
    let file = File::open(path).map_err(|err| gfm_types::GfmError::io(path, err))?;
    let mut lines = BufReader::new(file).lines();
    let Some(line) = lines.next() else {
        return Ok(report(
            kind,
            path,
            ArchiveSchemaStatus::Unsupported,
            None,
            Some("empty archive".to_string()),
        ));
    };
    let schema = line.map_err(|err| gfm_types::GfmError::io(path, err))?;
    let status = if current.contains(&schema.as_str()) {
        ArchiveSchemaStatus::Current
    } else if legacy.contains(&schema.as_str()) {
        ArchiveSchemaStatus::Legacy
    } else {
        return Ok(report(
            kind,
            path,
            ArchiveSchemaStatus::Unsupported,
            Some(schema),
            None,
        ));
    };
    validated_report(kind, path, status, Some(schema), validate)
}

fn inspect_magic_schema(
    kind: ArchiveSchemaKind,
    path: &Path,
    current: &[&[u8]],
    legacy: &[&[u8]],
    validate: impl FnOnce() -> Result<()>,
) -> Result<ArchiveSchemaReport> {
    let max_len = current
        .iter()
        .chain(legacy.iter())
        .map(|magic| magic.len())
        .max()
        .unwrap_or(0);
    let mut bytes = vec![0; max_len];
    let mut file = File::open(path).map_err(|err| gfm_types::GfmError::io(path, err))?;
    let len = file
        .read(&mut bytes)
        .map_err(|err| gfm_types::GfmError::io(path, err))?;
    bytes.truncate(len);

    if let Some(schema) = matching_magic(&bytes, current) {
        return validated_report(
            kind,
            path,
            ArchiveSchemaStatus::Current,
            Some(schema),
            validate,
        );
    }
    if let Some(schema) = matching_magic(&bytes, legacy) {
        return validated_report(
            kind,
            path,
            ArchiveSchemaStatus::Legacy,
            Some(schema),
            validate,
        );
    }
    Ok(report(
        kind,
        path,
        ArchiveSchemaStatus::Unsupported,
        None,
        Some("unsupported archive header".to_string()),
    ))
}

fn validated_report(
    kind: ArchiveSchemaKind,
    path: &Path,
    status: ArchiveSchemaStatus,
    schema: Option<String>,
    validate: impl FnOnce() -> Result<()>,
) -> Result<ArchiveSchemaReport> {
    match validate() {
        Ok(()) => Ok(report(kind, path, status, schema, None)),
        Err(err) => Ok(report(
            kind,
            path,
            ArchiveSchemaStatus::Unreadable,
            schema,
            Some(err.to_string()),
        )),
    }
}

fn matching_magic(bytes: &[u8], candidates: &[&[u8]]) -> Option<String> {
    candidates
        .iter()
        .find(|magic| bytes.starts_with(magic))
        .map(|magic| String::from_utf8_lossy(magic).trim_end().to_string())
}

fn report(
    kind: ArchiveSchemaKind,
    path: &Path,
    status: ArchiveSchemaStatus,
    schema: Option<String>,
    detail: Option<String>,
) -> ArchiveSchemaReport {
    ArchiveSchemaReport {
        kind,
        path: path.to_path_buf(),
        status,
        schema,
        current_schema: kind.current_schema(),
        detail,
    }
}

pub(crate) fn backup_archive(path: &Path, backup_dir: &Path, label: &str) -> Result<PathBuf> {
    fs::create_dir_all(backup_dir).map_err(|err| GfmError::io(backup_dir, err))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("records.gfmidx");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let backup_path = backup_dir.join(format!("{name}.{label}.{}.{}", std::process::id(), nanos));
    fs::copy(path, &backup_path).map_err(|err| GfmError::io(path, err))?;
    if let Ok(file) = File::open(&backup_path) {
        let _ = file.sync_all();
    }
    if let Ok(dir) = File::open(backup_dir) {
        let _ = dir.sync_all();
    }
    Ok(backup_path)
}

pub(crate) fn escape_field(value: &str) -> String {
    value.replace(['\t', '\n', '\r'], " ")
}

impl fmt::Display for ArchiveSchemaStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
