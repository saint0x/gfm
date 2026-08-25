use crate::{
    dictionary_terms_from_records, fuzzy_postings_from_records, inspect_archive_schema,
    metadata_postings_from_records, prefix_postings_from_records, schema, sidecar_kind_name,
    write_dictionary, write_fuzzy_postings, write_metadata_postings, write_prefix_postings,
    write_record_columns, ArchiveSchemaKind, ArchiveSchemaReport, ArchiveSchemaStatus,
    MmapRecordArchive, SidecarKind,
};
use gfm_types::{FileRecord, GfmError, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DerivedSidecarRebuildAction {
    Ready,
    Rebuild,
    CannotRebuild,
}

impl DerivedSidecarRebuildAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Rebuild => "rebuild",
            Self::CannotRebuild => "cannot-rebuild",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedSidecarRebuildPlan {
    pub action: DerivedSidecarRebuildAction,
    pub kind: SidecarKind,
    pub records: ArchiveSchemaReport,
    pub sidecar: ArchiveSchemaReport,
    pub detail: Option<String>,
}

impl DerivedSidecarRebuildPlan {
    pub fn as_tsv(&self) -> String {
        format!(
            "derived-sidecar-rebuild-plan\taction={}\tkind={}\trecords-status={}\tsidecar-status={}\tsidecar-schema={}\tcurrent={}\trecords={}\tsidecar={}\tdetail={}",
            self.action.as_str(),
            sidecar_kind_name(self.kind),
            self.records.status.as_str(),
            self.sidecar.status.as_str(),
            self.sidecar.schema.as_deref().unwrap_or("-"),
            self.sidecar.current_schema,
            schema::escape_field(&self.records.path.display().to_string()),
            schema::escape_field(&self.sidecar.path.display().to_string()),
            self.detail
                .as_deref()
                .map(schema::escape_field)
                .unwrap_or("-".to_string())
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedSidecarRebuild {
    pub before: DerivedSidecarRebuildPlan,
    pub after: ArchiveSchemaReport,
    pub rebuilt_records: usize,
    pub backup_path: Option<PathBuf>,
}

impl DerivedSidecarRebuild {
    pub fn as_tsv(&self) -> String {
        format!(
            "derived-sidecar-rebuild\trebuilt-records={}\tkind={}\trecords-status={}\tbefore-status={}\tafter-status={}\tbackup={}\tpath={}",
            self.rebuilt_records,
            sidecar_kind_name(self.before.kind),
            self.before.records.status.as_str(),
            self.before.sidecar.status.as_str(),
            self.after.status.as_str(),
            self.backup_path
                .as_ref()
                .map(|path| schema::escape_field(&path.display().to_string()))
                .unwrap_or("-".to_string()),
            schema::escape_field(&self.after.path.display().to_string())
        )
    }
}

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

pub fn plan_derived_sidecar_rebuild(
    records_path: impl AsRef<Path>,
    kind: SidecarKind,
    sidecar_path: impl AsRef<Path>,
) -> DerivedSidecarRebuildPlan {
    let records = inspect_archive_schema(ArchiveSchemaKind::Records, records_path);
    let sidecar = inspect_archive_schema(archive_kind_for_sidecar(kind), sidecar_path);
    let records_readable = matches!(
        records.status,
        ArchiveSchemaStatus::Current | ArchiveSchemaStatus::Legacy
    );
    let (action, detail) = if !records_readable {
        (
            DerivedSidecarRebuildAction::CannotRebuild,
            Some(
                match records.status {
                    ArchiveSchemaStatus::Missing => {
                        "missing record archive prevents derived sidecar rebuild"
                    }
                    ArchiveSchemaStatus::Unsupported => {
                        "unsupported record archive prevents derived sidecar rebuild"
                    }
                    ArchiveSchemaStatus::Unreadable => {
                        "unreadable record archive prevents derived sidecar rebuild"
                    }
                    ArchiveSchemaStatus::Current | ArchiveSchemaStatus::Legacy => {
                        "record archive is not readable"
                    }
                }
                .to_string(),
            ),
        )
    } else {
        match sidecar.status {
            ArchiveSchemaStatus::Current => (
                DerivedSidecarRebuildAction::Ready,
                Some(format!(
                    "{} sidecar is already current",
                    sidecar_kind_name(kind)
                )),
            ),
            ArchiveSchemaStatus::Legacy => (
                DerivedSidecarRebuildAction::Rebuild,
                Some(format!(
                    "legacy {} sidecar is derived data and will be rebuilt from durable records",
                    sidecar_kind_name(kind)
                )),
            ),
            ArchiveSchemaStatus::Missing => (
                DerivedSidecarRebuildAction::Rebuild,
                Some(format!(
                    "missing {} sidecar will be rebuilt from durable records",
                    sidecar_kind_name(kind)
                )),
            ),
            ArchiveSchemaStatus::Unsupported => (
                DerivedSidecarRebuildAction::Rebuild,
                Some(format!(
                    "unsupported {} sidecar will be backed up and rebuilt from durable records",
                    sidecar_kind_name(kind)
                )),
            ),
            ArchiveSchemaStatus::Unreadable => (
                DerivedSidecarRebuildAction::Rebuild,
                Some(format!(
                    "unreadable {} sidecar will be backed up and rebuilt from durable records",
                    sidecar_kind_name(kind)
                )),
            ),
        }
    };
    DerivedSidecarRebuildPlan {
        action,
        kind,
        records,
        sidecar,
        detail,
    }
}

pub fn rebuild_derived_sidecar(
    records_path: impl AsRef<Path>,
    kind: SidecarKind,
    sidecar_path: impl AsRef<Path>,
    backup_dir: impl AsRef<Path>,
) -> Result<DerivedSidecarRebuild> {
    let records_path = records_path.as_ref();
    let sidecar_path = sidecar_path.as_ref();
    let backup_dir = backup_dir.as_ref();
    let before = plan_derived_sidecar_rebuild(records_path, kind, sidecar_path);
    match before.action {
        DerivedSidecarRebuildAction::Ready => {
            return Ok(DerivedSidecarRebuild {
                after: before.sidecar.clone(),
                before,
                rebuilt_records: 0,
                backup_path: None,
            });
        }
        DerivedSidecarRebuildAction::CannotRebuild => {
            return Err(GfmError::Format(format!(
                "{} cannot be rebuilt: {}",
                sidecar_path.display(),
                before
                    .detail
                    .as_deref()
                    .unwrap_or("record archive is not readable")
            )));
        }
        DerivedSidecarRebuildAction::Rebuild => {}
    }

    let records = MmapRecordArchive::open(records_path)?.records()?;
    let backup_path = if sidecar_path.exists() {
        let label = match before.sidecar.status {
            ArchiveSchemaStatus::Legacy => "legacy",
            ArchiveSchemaStatus::Unsupported => "unsupported",
            ArchiveSchemaStatus::Unreadable => "unreadable",
            ArchiveSchemaStatus::Current | ArchiveSchemaStatus::Missing => sidecar_kind_name(kind),
        };
        Some(schema::backup_archive(sidecar_path, backup_dir, label)?)
    } else {
        None
    };
    write_derived_sidecar(kind, sidecar_path, &records)?;
    let after = inspect_archive_schema(archive_kind_for_sidecar(kind), sidecar_path);
    if after.status != ArchiveSchemaStatus::Current {
        return Err(GfmError::Format(format!(
            "{} rebuild produced {} instead of current schema",
            sidecar_path.display(),
            after.status.as_str()
        )));
    }
    Ok(DerivedSidecarRebuild {
        before,
        after,
        rebuilt_records: records.len(),
        backup_path,
    })
}

pub fn plan_columns_archive_rebuild(
    records_path: impl AsRef<Path>,
    columns_path: impl AsRef<Path>,
) -> ColumnsArchiveRebuildPlan {
    let plan = plan_derived_sidecar_rebuild(records_path, SidecarKind::Columns, columns_path);
    ColumnsArchiveRebuildPlan {
        action: columns_action_from_derived(plan.action),
        records: plan.records,
        columns: plan.sidecar,
        detail: plan.detail,
    }
}

pub fn rebuild_columns_archive(
    records_path: impl AsRef<Path>,
    columns_path: impl AsRef<Path>,
    backup_dir: impl AsRef<Path>,
) -> Result<ColumnsArchiveRebuild> {
    let rebuild =
        rebuild_derived_sidecar(records_path, SidecarKind::Columns, columns_path, backup_dir)?;
    Ok(ColumnsArchiveRebuild {
        before: ColumnsArchiveRebuildPlan {
            action: columns_action_from_derived(rebuild.before.action),
            records: rebuild.before.records,
            columns: rebuild.before.sidecar,
            detail: rebuild.before.detail,
        },
        after: rebuild.after,
        rebuilt_records: rebuild.rebuilt_records,
        backup_path: rebuild.backup_path,
    })
}

fn columns_action_from_derived(action: DerivedSidecarRebuildAction) -> ColumnsArchiveRebuildAction {
    match action {
        DerivedSidecarRebuildAction::Ready => ColumnsArchiveRebuildAction::Ready,
        DerivedSidecarRebuildAction::Rebuild => ColumnsArchiveRebuildAction::Rebuild,
        DerivedSidecarRebuildAction::CannotRebuild => ColumnsArchiveRebuildAction::CannotRebuild,
    }
}

fn archive_kind_for_sidecar(kind: SidecarKind) -> ArchiveSchemaKind {
    match kind {
        SidecarKind::Columns => ArchiveSchemaKind::Columns,
        SidecarKind::Metadata => ArchiveSchemaKind::Metadata,
        SidecarKind::Prefixes => ArchiveSchemaKind::Prefixes,
        SidecarKind::Fuzzy => ArchiveSchemaKind::Fuzzy,
        SidecarKind::Dictionary => ArchiveSchemaKind::Dictionary,
    }
}

fn write_derived_sidecar(kind: SidecarKind, path: &Path, records: &[FileRecord]) -> Result<()> {
    match kind {
        SidecarKind::Columns => write_record_columns(path, records),
        SidecarKind::Metadata => {
            write_metadata_postings(path, &metadata_postings_from_records(records))
        }
        SidecarKind::Prefixes => {
            write_prefix_postings(path, &prefix_postings_from_records(records))
        }
        SidecarKind::Fuzzy => write_fuzzy_postings(path, &fuzzy_postings_from_records(records)),
        SidecarKind::Dictionary => write_dictionary(path, &dictionary_terms_from_records(records)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        read_dictionary, read_metadata_postings, write_records, MmapFuzzyArchive,
        MmapPrefixArchive, MmapRecordColumns,
    };
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

    #[test]
    fn rebuilds_all_non_column_search_sidecars_from_durable_records() {
        let dir = temp_dir("gfm-derived-sidecar-rebuild-all");
        let records = dir.join("records.gfmidx");
        write_records(&records, &[record()]).unwrap();

        for (kind, name) in [
            (SidecarKind::Metadata, "records.gfmmeta"),
            (SidecarKind::Prefixes, "records.gfmprefix"),
            (SidecarKind::Fuzzy, "records.gfmfuzzy"),
            (SidecarKind::Dictionary, "records.gfmdict"),
        ] {
            let sidecar = dir.join(name);
            let backup = dir.join(format!("backup-{}", sidecar_kind_name(kind)));
            let plan = plan_derived_sidecar_rebuild(&records, kind, &sidecar);
            assert_eq!(plan.action, DerivedSidecarRebuildAction::Rebuild);
            assert_eq!(plan.sidecar.status, ArchiveSchemaStatus::Missing);

            let rebuild = rebuild_derived_sidecar(&records, kind, &sidecar, &backup).unwrap();

            assert_eq!(rebuild.rebuilt_records, 1);
            assert_eq!(rebuild.after.status, ArchiveSchemaStatus::Current);
            assert!(rebuild.backup_path.is_none());
            assert!(!backup.exists());
        }

        assert!(!read_metadata_postings(dir.join("records.gfmmeta"))
            .unwrap()
            .is_empty());
        assert!(!MmapPrefixArchive::open(dir.join("records.gfmprefix"))
            .unwrap()
            .ids_for("instant")
            .unwrap()
            .is_empty());
        assert!(!MmapFuzzyArchive::open(dir.join("records.gfmfuzzy"))
            .unwrap()
            .terms_for("istant")
            .unwrap()
            .is_empty());
        assert!(read_dictionary(dir.join("records.gfmdict"))
            .unwrap()
            .iter()
            .any(|term| term.contains("instant")));

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn backs_up_unreadable_derived_sidecar_before_rebuild() {
        let dir = temp_dir("gfm-derived-sidecar-rebuild-unreadable");
        let records = dir.join("records.gfmidx");
        let prefixes = dir.join("records.gfmprefix");
        let backup = dir.join("backup");
        write_records(&records, &[record()]).unwrap();
        std::fs::write(&prefixes, b"gfm-prefix-v1\n").unwrap();

        let plan = plan_derived_sidecar_rebuild(&records, SidecarKind::Prefixes, &prefixes);
        assert_eq!(plan.action, DerivedSidecarRebuildAction::Rebuild);
        assert_eq!(plan.sidecar.status, ArchiveSchemaStatus::Unreadable);

        let rebuild =
            rebuild_derived_sidecar(&records, SidecarKind::Prefixes, &prefixes, &backup).unwrap();

        assert_eq!(rebuild.after.status, ArchiveSchemaStatus::Current);
        assert!(rebuild.backup_path.unwrap().exists());
        assert!(!MmapPrefixArchive::open(&prefixes)
            .unwrap()
            .ids_for("instant")
            .unwrap()
            .is_empty());

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
