use crate::{
    inspect_archive_schema, schema, write_record_columns, ArchiveSchemaKind, ArchiveSchemaReport,
    ArchiveSchemaStatus, MmapRecordArchive,
};
use gfm_types::{GfmError, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnsArchiveRebuildAction {
    Ready,
    Rebuild,
    CannotRebuild,
}

impl ColumnsArchiveRebuildAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Rebuild => "rebuild",
            Self::CannotRebuild => "cannot-rebuild",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnsArchiveRebuildPlan {
    pub action: ColumnsArchiveRebuildAction,
    pub records: ArchiveSchemaReport,
    pub columns: ArchiveSchemaReport,
    pub detail: Option<String>,
}

impl ColumnsArchiveRebuildPlan {
    pub fn as_tsv(&self) -> String {
        format!(
            "columns-archive-rebuild-plan\taction={}\trecords-status={}\tcolumns-status={}\tcolumns-schema={}\tcurrent={}\trecords={}\tcolumns={}\tdetail={}",
            self.action.as_str(),
            self.records.status.as_str(),
            self.columns.status.as_str(),
            self.columns.schema.as_deref().unwrap_or("-"),
            self.columns.current_schema,
            schema::escape_field(&self.records.path.display().to_string()),
            schema::escape_field(&self.columns.path.display().to_string()),
            self.detail
                .as_deref()
                .map(schema::escape_field)
                .unwrap_or("-".to_string())
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnsArchiveRebuild {
    pub before: ColumnsArchiveRebuildPlan,
    pub after: ArchiveSchemaReport,
    pub rebuilt_records: usize,
    pub backup_path: Option<PathBuf>,
}

impl ColumnsArchiveRebuild {
    pub fn as_tsv(&self) -> String {
        format!(
            "columns-archive-rebuild\trebuilt-records={}\trecords-status={}\tbefore-status={}\tafter-status={}\tbackup={}\tpath={}",
            self.rebuilt_records,
            self.before.records.status.as_str(),
            self.before.columns.status.as_str(),
            self.after.status.as_str(),
            self.backup_path
                .as_ref()
                .map(|path| schema::escape_field(&path.display().to_string()))
                .unwrap_or("-".to_string()),
            schema::escape_field(&self.after.path.display().to_string())
        )
    }
}

pub fn plan_columns_archive_rebuild(
    records_path: impl AsRef<Path>,
    columns_path: impl AsRef<Path>,
) -> ColumnsArchiveRebuildPlan {
    let records = inspect_archive_schema(ArchiveSchemaKind::Records, records_path);
    let columns = inspect_archive_schema(ArchiveSchemaKind::Columns, columns_path);
    let records_readable = matches!(
        records.status,
        ArchiveSchemaStatus::Current | ArchiveSchemaStatus::Legacy
    );
    let (action, detail) = if !records_readable {
        (
            ColumnsArchiveRebuildAction::CannotRebuild,
            Some(
                match records.status {
                    ArchiveSchemaStatus::Missing => {
                        "missing record archive prevents derived columns rebuild"
                    }
                    ArchiveSchemaStatus::Unsupported => {
                        "unsupported record archive prevents derived columns rebuild"
                    }
                    ArchiveSchemaStatus::Unreadable => {
                        "unreadable record archive prevents derived columns rebuild"
                    }
                    ArchiveSchemaStatus::Current | ArchiveSchemaStatus::Legacy => {
                        "record archive is not readable"
                    }
                }
                .to_string(),
            ),
        )
    } else {
        match columns.status {
            ArchiveSchemaStatus::Current => (
                ColumnsArchiveRebuildAction::Ready,
                Some("columns archive is already current".to_string()),
            ),
            ArchiveSchemaStatus::Legacy => (
                ColumnsArchiveRebuildAction::Rebuild,
                Some(
                    "legacy columns are derived data and will be rebuilt from durable records"
                        .to_string(),
                ),
            ),
            ArchiveSchemaStatus::Missing => (
                ColumnsArchiveRebuildAction::Rebuild,
                Some("missing columns will be rebuilt from durable records".to_string()),
            ),
            ArchiveSchemaStatus::Unsupported => (
                ColumnsArchiveRebuildAction::Rebuild,
                Some(
                    "unsupported columns will be backed up and rebuilt from durable records"
                        .to_string(),
                ),
            ),
            ArchiveSchemaStatus::Unreadable => (
                ColumnsArchiveRebuildAction::Rebuild,
                Some(
                    "unreadable columns will be backed up and rebuilt from durable records"
                        .to_string(),
                ),
            ),
        }
    };
    ColumnsArchiveRebuildPlan {
        action,
        records,
        columns,
        detail,
    }
}

pub fn rebuild_columns_archive(
    records_path: impl AsRef<Path>,
    columns_path: impl AsRef<Path>,
    backup_dir: impl AsRef<Path>,
) -> Result<ColumnsArchiveRebuild> {
    let records_path = records_path.as_ref();
    let columns_path = columns_path.as_ref();
    let backup_dir = backup_dir.as_ref();
    let before = plan_columns_archive_rebuild(records_path, columns_path);
    match before.action {
        ColumnsArchiveRebuildAction::Ready => {
            return Ok(ColumnsArchiveRebuild {
                after: before.columns.clone(),
                before,
                rebuilt_records: 0,
                backup_path: None,
            });
        }
        ColumnsArchiveRebuildAction::CannotRebuild => {
            return Err(GfmError::Format(format!(
                "{} cannot be rebuilt: {}",
                columns_path.display(),
                before
                    .detail
                    .as_deref()
                    .unwrap_or("record archive is not readable")
            )));
        }
        ColumnsArchiveRebuildAction::Rebuild => {}
    }

    let records = MmapRecordArchive::open(records_path)?.records()?;
    let backup_path = if columns_path.exists() {
        let label = match before.columns.status {
            ArchiveSchemaStatus::Legacy => "legacy",
            ArchiveSchemaStatus::Unsupported => "unsupported",
            ArchiveSchemaStatus::Unreadable => "unreadable",
            ArchiveSchemaStatus::Current | ArchiveSchemaStatus::Missing => "columns",
        };
        Some(schema::backup_archive(columns_path, backup_dir, label)?)
    } else {
        None
    };
    write_record_columns(columns_path, &records)?;
    let after = inspect_archive_schema(ArchiveSchemaKind::Columns, columns_path);
    if after.status != ArchiveSchemaStatus::Current {
        return Err(GfmError::Format(format!(
            "{} rebuild produced {} instead of current schema",
            columns_path.display(),
            after.status.as_str()
        )));
    }
    Ok(ColumnsArchiveRebuild {
        before,
        after,
        rebuilt_records: records.len(),
        backup_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{write_records, MmapRecordColumns};
    use gfm_types::{FileId, FileKind, FileRecord, VolumeId};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn rebuilds_missing_columns_archive_from_durable_records() {
        let dir = temp_dir("gfm-schema-columns-rebuild-missing");
        let records = dir.join("records.gfmidx");
        let columns = dir.join("records.gfmcols");
        let backup = dir.join("backup");
        write_records(&records, &[record()]).unwrap();

        let plan = plan_columns_archive_rebuild(&records, &columns);
        assert_eq!(plan.action, ColumnsArchiveRebuildAction::Rebuild);
        assert_eq!(plan.columns.status, ArchiveSchemaStatus::Missing);

        let rebuild = rebuild_columns_archive(&records, &columns, &backup).unwrap();

        assert_eq!(rebuild.rebuilt_records, 1);
        assert_eq!(rebuild.after.status, ArchiveSchemaStatus::Current);
        assert!(rebuild.backup_path.is_none());
        assert!(!backup.exists());
        let columns = MmapRecordColumns::open(&columns).unwrap();
        assert_eq!(columns.len(), 1);
        assert!(columns.is_checksummed());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rebuilds_legacy_columns_archive_with_backup() {
        let dir = temp_dir("gfm-schema-columns-rebuild-legacy");
        let records = dir.join("records.gfmidx");
        let columns = dir.join("legacy.gfmcols");
        let backup = dir.join("backup");
        write_records(&records, &[record()]).unwrap();
        crate::columns::write_record_columns_v1(&columns, &[record()]).unwrap();

        let plan = plan_columns_archive_rebuild(&records, &columns);
        assert_eq!(plan.action, ColumnsArchiveRebuildAction::Rebuild);
        assert_eq!(plan.columns.status, ArchiveSchemaStatus::Legacy);

        let rebuild = rebuild_columns_archive(&records, &columns, &backup).unwrap();

        assert_eq!(rebuild.rebuilt_records, 1);
        assert_eq!(rebuild.after.status, ArchiveSchemaStatus::Current);
        let backup_path = rebuild.backup_path.unwrap();
        assert!(backup_path.exists());
        assert_eq!(
            inspect_archive_schema(ArchiveSchemaKind::Columns, &backup_path).status,
            ArchiveSchemaStatus::Legacy
        );
        assert_eq!(
            inspect_archive_schema(ArchiveSchemaKind::Columns, &columns)
                .schema
                .as_deref(),
            Some("gfm-record-columns-v2")
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rebuilds_unreadable_columns_archive_with_backup() {
        let dir = temp_dir("gfm-schema-columns-rebuild-unreadable");
        let records = dir.join("records.gfmidx");
        let columns = dir.join("bad.gfmcols");
        let backup = dir.join("backup");
        write_records(&records, &[record()]).unwrap();
        std::fs::write(&columns, b"gfm-record-columns-v2\n").unwrap();

        let plan = plan_columns_archive_rebuild(&records, &columns);
        assert_eq!(plan.action, ColumnsArchiveRebuildAction::Rebuild);
        assert_eq!(plan.columns.status, ArchiveSchemaStatus::Unreadable);

        let rebuild = rebuild_columns_archive(&records, &columns, &backup).unwrap();

        assert_eq!(rebuild.after.status, ArchiveSchemaStatus::Current);
        assert!(rebuild.backup_path.unwrap().exists());
        assert!(MmapRecordColumns::open(&columns).unwrap().is_checksummed());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn refuses_columns_rebuild_without_readable_records() {
        let dir = temp_dir("gfm-schema-columns-rebuild-no-records");
        let records = dir.join("records.gfmidx");
        let columns = dir.join("records.gfmcols");
        let backup = dir.join("backup");
        std::fs::write(&records, "not-gfm\n").unwrap();

        let plan = plan_columns_archive_rebuild(&records, &columns);
        assert_eq!(plan.action, ColumnsArchiveRebuildAction::CannotRebuild);
        assert!(rebuild_columns_archive(&records, &columns, &backup).is_err());
        assert!(!columns.exists());

        std::fs::remove_dir_all(dir).unwrap();
    }

    fn record() -> FileRecord {
        FileRecord {
            id: FileId::new(VolumeId(1), 2),
            parent: Some(FileId::new(VolumeId(1), 1)),
            path: PathBuf::from("/tmp/rebuild/Instant Search.md"),
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
}
