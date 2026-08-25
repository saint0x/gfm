use crate::{
    read_content_postings, read_metadata_postings, read_records, write_content_postings,
    write_metadata_postings, write_records, ContentArchive, ContentArchiveManifest,
    MmapContentArchive, MmapDictionary, MmapFuzzyArchive, MmapMetadataArchive, MmapPrefixArchive,
    MmapRecordArchive, MmapRecordColumns,
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

fn backup_archive(path: &Path, backup_dir: &Path, label: &str) -> Result<PathBuf> {
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

fn escape_field(value: &str) -> String {
    value.replace(['\t', '\n', '\r'], " ")
}

impl fmt::Display for ArchiveSchemaStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        metadata_postings_from_records, prefix_postings_from_records, write_content_postings,
        write_dictionary, write_fuzzy_postings, write_metadata_postings, write_prefix_postings,
        write_record_columns, write_records, ContentArchiveManifestEntry, ContentMergeTier,
        MetadataField, MetadataPosting,
    };
    use gfm_types::{ContentPosting, FileId, FileKind, FileRecord, VolumeId};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn inspects_current_archive_schemas_with_real_readers() {
        let records = temp_path("gfm-schema-records", "gfmidx");
        let columns = temp_path("gfm-schema-columns", "gfmcols");
        let metadata = temp_path("gfm-schema-metadata", "gfmmeta");
        let prefixes = temp_path("gfm-schema-prefixes", "gfmprefix");
        let fuzzy = temp_path("gfm-schema-fuzzy", "gfmfuzzy");
        let dictionary = temp_path("gfm-schema-dictionary", "gfmdict");
        let content = temp_path("gfm-schema-content", "gfmcontent");
        let manifest = temp_path("gfm-schema-manifest", "gfmmanifest");
        let rows = vec![record()];

        write_records(&records, &rows).unwrap();
        write_record_columns(&columns, &rows).unwrap();
        write_metadata_postings(&metadata, &metadata_postings_from_records(&rows)).unwrap();
        write_prefix_postings(&prefixes, &prefix_postings_from_records(&rows)).unwrap();
        write_fuzzy_postings(&fuzzy, &crate::fuzzy_postings_from_records(&rows)).unwrap();
        write_dictionary(&dictionary, &crate::dictionary_terms_from_records(&rows)).unwrap();
        write_content_postings(
            &content,
            &[ContentPosting {
                term: "instant".to_string(),
                ids: vec![FileId::new(VolumeId(1), 2)],
                positions: Vec::new(),
            }],
        )
        .unwrap();
        ContentArchiveManifest::new(vec![ContentArchiveManifestEntry {
            tier: ContentMergeTier::Hot,
            path: content.clone(),
        }])
        .unwrap()
        .write(&manifest)
        .unwrap();

        for (kind, path) in [
            (ArchiveSchemaKind::Records, records.as_path()),
            (ArchiveSchemaKind::Columns, columns.as_path()),
            (ArchiveSchemaKind::Metadata, metadata.as_path()),
            (ArchiveSchemaKind::Prefixes, prefixes.as_path()),
            (ArchiveSchemaKind::Fuzzy, fuzzy.as_path()),
            (ArchiveSchemaKind::Dictionary, dictionary.as_path()),
            (ArchiveSchemaKind::Content, content.as_path()),
            (ArchiveSchemaKind::ContentManifest, manifest.as_path()),
        ] {
            let report = inspect_archive_schema(kind, path);
            assert_eq!(report.status, ArchiveSchemaStatus::Current, "{report:?}");
            assert_eq!(report.schema.as_deref(), Some(kind.current_schema()));
        }

        for path in [
            records, columns, metadata, prefixes, fuzzy, dictionary, content, manifest,
        ] {
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn classifies_legacy_missing_unsupported_and_unreadable_archives() {
        let legacy = temp_path("gfm-schema-legacy", "gfmidx");
        let missing = temp_path("gfm-schema-missing", "gfmidx");
        let unsupported = temp_path("gfm-schema-unsupported", "gfmidx");
        let corrupt = temp_path("gfm-schema-corrupt", "gfmprefix");

        std::fs::write(
            &legacy,
            "gfm-store-v1\n1\t2\t0\tf\t1\t0\t0\t0\t0\t/tmp/legacy.txt\n",
        )
        .unwrap();
        std::fs::write(&unsupported, "not-gfm\nbody\n").unwrap();
        std::fs::write(&corrupt, PREFIX_CURRENT).unwrap();

        assert_eq!(
            inspect_archive_schema(ArchiveSchemaKind::Records, &legacy).status,
            ArchiveSchemaStatus::Legacy
        );
        assert_eq!(
            inspect_archive_schema(ArchiveSchemaKind::Records, &missing).status,
            ArchiveSchemaStatus::Missing
        );
        assert_eq!(
            inspect_archive_schema(ArchiveSchemaKind::Records, &unsupported).status,
            ArchiveSchemaStatus::Unsupported
        );
        let corrupt_report = inspect_archive_schema(ArchiveSchemaKind::Prefixes, &corrupt);
        assert_eq!(corrupt_report.status, ArchiveSchemaStatus::Unreadable);
        assert_eq!(corrupt_report.schema.as_deref(), Some("gfm-prefix-v1"));

        std::fs::remove_file(legacy).unwrap();
        std::fs::remove_file(unsupported).unwrap();
        std::fs::remove_file(corrupt).unwrap();
    }

    #[test]
    fn migrates_legacy_record_archive_to_current_schema_with_backup() {
        let dir = temp_dir("gfm-schema-record-migration");
        let records = dir.join("legacy.gfmidx");
        let backup = dir.join("backup");
        std::fs::write(
            &records,
            "gfm-store-v1\n1\t2\t0\tf\t1\t0\t0\t0\t0\t/tmp/legacy.txt\n",
        )
        .unwrap();

        let plan = plan_record_archive_migration(&records);
        assert_eq!(plan.action, RecordArchiveMigrationAction::Migrate);

        let migration = migrate_record_archive(&records, &backup).unwrap();

        assert_eq!(migration.migrated_records, 1);
        assert_eq!(migration.after.status, ArchiveSchemaStatus::Current);
        assert_eq!(read_records(&records).unwrap()[0].name, "legacy.txt");
        let backup_path = migration.backup_path.unwrap();
        assert!(backup_path.exists());
        assert_eq!(
            inspect_archive_schema(ArchiveSchemaKind::Records, &backup_path).status,
            ArchiveSchemaStatus::Legacy
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migrates_legacy_content_archive_to_current_schema_with_backup() {
        let dir = temp_dir("gfm-schema-content-migration");
        let content = dir.join("legacy.gfmcontent");
        let backup = dir.join("backup");
        let postings = vec![
            ContentPosting {
                term: "alpha".to_string(),
                ids: vec![FileId::new(VolumeId(1), 2), FileId::new(VolumeId(1), 4)],
                positions: Vec::new(),
            },
            ContentPosting {
                term: "beta".to_string(),
                ids: vec![FileId::new(VolumeId(2), 1)],
                positions: Vec::new(),
            },
        ];
        write_legacy_content_archive(&content, &postings);

        let plan = plan_content_archive_migration(&content);
        assert_eq!(plan.action, ContentArchiveMigrationAction::Migrate);

        let migration = migrate_content_archive(&content, &backup).unwrap();

        assert_eq!(migration.migrated_postings, 2);
        assert_eq!(migration.after.status, ArchiveSchemaStatus::Current);
        assert_eq!(read_content_postings(&content).unwrap(), postings);
        assert!(MmapContentArchive::open(&content).unwrap().is_checksummed());
        let backup_path = migration.backup_path.unwrap();
        assert!(backup_path.exists());
        assert_eq!(
            inspect_archive_schema(ArchiveSchemaKind::Content, &backup_path).status,
            ArchiveSchemaStatus::Legacy
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migrates_legacy_metadata_archive_to_current_schema_with_backup() {
        let dir = temp_dir("gfm-schema-metadata-migration");
        let metadata = dir.join("legacy.gfmmeta");
        let backup = dir.join("backup");
        let postings = vec![
            MetadataPosting {
                field: MetadataField::Tag,
                term: "important".to_string(),
                ids: vec![FileId::new(VolumeId(1), 2), FileId::new(VolumeId(1), 4)],
            },
            MetadataPosting {
                field: MetadataField::Comment,
                term: "handoff".to_string(),
                ids: vec![FileId::new(VolumeId(2), 1)],
            },
        ];
        write_legacy_metadata_archive(&metadata, &postings);

        let plan = plan_metadata_archive_migration(&metadata);
        assert_eq!(plan.action, MetadataArchiveMigrationAction::Migrate);

        let migration = migrate_metadata_archive(&metadata, &backup).unwrap();

        assert_eq!(migration.migrated_postings, 2);
        assert_eq!(migration.after.status, ArchiveSchemaStatus::Current);
        assert_eq!(read_metadata_postings(&metadata).unwrap(), postings);
        assert!(MmapMetadataArchive::open(&metadata)
            .unwrap()
            .is_checksummed());
        let backup_path = migration.backup_path.unwrap();
        assert!(backup_path.exists());
        assert_eq!(
            inspect_archive_schema(ArchiveSchemaKind::Metadata, &backup_path).status,
            ArchiveSchemaStatus::Legacy
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn current_record_archive_migration_is_noop() {
        let dir = temp_dir("gfm-schema-record-migration-current");
        let records = dir.join("current.gfmidx");
        let backup = dir.join("backup");
        write_records(&records, &[record()]).unwrap();

        let migration = migrate_record_archive(&records, &backup).unwrap();

        assert_eq!(migration.migrated_records, 0);
        assert_eq!(migration.after.status, ArchiveSchemaStatus::Current);
        assert!(migration.backup_path.is_none());
        assert!(!backup.exists());

        std::fs::remove_dir_all(dir).unwrap();
    }

    fn record() -> FileRecord {
        FileRecord {
            id: FileId::new(VolumeId(1), 2),
            parent: Some(FileId::new(VolumeId(1), 1)),
            path: PathBuf::from("/tmp/schema/Instant Search.md"),
            name: "Instant Search.md".to_string(),
            kind: FileKind::File,
            len: 128,
            mode: 0o100644,
            owner: 501,
            group: 20,
            xattrs_digest: 7,
            created: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
            modified: Some(UNIX_EPOCH + Duration::from_secs(2)),
            changed: None,
            hidden: false,
            tags: vec!["fast".to_string()],
            finder_comment: Some("instant lookup".to_string()),
        }
    }

    fn temp_path(prefix: &str, extension: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!("{prefix}-{nanos}.{extension}"));
        path
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!("{prefix}-{nanos}"));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_legacy_content_archive(path: &Path, postings: &[ContentPosting]) {
        let mut bytes = Vec::new();
        bytes.extend(b"gfm-content-v1\n");
        push_varint(&mut bytes, postings.len() as u64);
        for posting in postings {
            push_varint(&mut bytes, posting.term.len() as u64);
            bytes.extend(posting.term.as_bytes());
            write_legacy_file_ids(&mut bytes, &posting.ids);
        }
        std::fs::write(path, bytes).unwrap();
    }

    fn write_legacy_metadata_archive(path: &Path, postings: &[MetadataPosting]) {
        let mut bytes = Vec::new();
        bytes.extend(b"gfm-metadata-v1\n");
        push_varint(&mut bytes, postings.len() as u64);
        let mut directory = Vec::new();
        let mut postings = postings.to_vec();
        postings.sort_by(|left, right| {
            (metadata_field_code(left.field), left.term.as_str())
                .cmp(&(metadata_field_code(right.field), right.term.as_str()))
        });
        for posting in &postings {
            let offset = bytes.len() as u64;
            write_legacy_metadata_posting(&mut bytes, posting);
            directory.push((
                posting.field,
                posting.term.clone(),
                offset,
                bytes.len() as u64 - offset,
            ));
        }
        let directory_offset = bytes.len() as u64;
        push_varint(&mut bytes, directory.len() as u64);
        for (field, term, offset, len) in directory {
            bytes.push(metadata_field_code(field));
            push_varint(&mut bytes, term.len() as u64);
            bytes.extend(term.as_bytes());
            push_varint(&mut bytes, offset);
            push_varint(&mut bytes, len);
        }
        bytes.extend(directory_offset.to_le_bytes());
        bytes.extend(b"gfm-metadata-index-v1\n");
        std::fs::write(path, bytes).unwrap();
    }

    fn write_legacy_metadata_posting(bytes: &mut Vec<u8>, posting: &MetadataPosting) {
        bytes.push(metadata_field_code(posting.field));
        push_varint(bytes, posting.term.len() as u64);
        bytes.extend(posting.term.as_bytes());
        write_legacy_file_ids(bytes, &posting.ids);
    }

    fn metadata_field_code(field: MetadataField) -> u8 {
        match field {
            MetadataField::Tag => b't',
            MetadataField::Comment => b'c',
        }
    }

    fn write_legacy_file_ids(bytes: &mut Vec<u8>, ids: &[FileId]) {
        let mut ids = ids.to_vec();
        ids.sort();
        push_varint(bytes, ids.len() as u64);
        let mut previous = FileId::new(VolumeId(0), 0);
        for id in ids {
            push_varint(bytes, id.volume.0.saturating_sub(previous.volume.0));
            let node_delta = if id.volume == previous.volume {
                id.node.saturating_sub(previous.node)
            } else {
                id.node
            };
            push_varint(bytes, node_delta);
            previous = id;
        }
    }

    fn push_varint(bytes: &mut Vec<u8>, mut value: u64) {
        while value >= 0x80 {
            bytes.push(((value as u8) & 0x7f) | 0x80);
            value >>= 7;
        }
        bytes.push(value as u8);
    }
}
