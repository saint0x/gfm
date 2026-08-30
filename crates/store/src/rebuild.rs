use crate::{
    content_manifest_recovery_action_name, dictionary_terms_from_records,
    fuzzy_postings_from_records, inspect_archive_schema, metadata_postings_from_records,
    plan_content_archive_migration, plan_content_manifest_recovery, plan_record_archive_migration,
    prefix_postings_from_records, schema, sidecar_kind_name, substring_postings_from_records,
    write_dictionary, write_fuzzy_postings_checked, write_metadata_postings,
    write_prefix_postings_checked, write_record_columns, write_substring_postings_checked,
    ArchiveSchemaKind, ArchiveSchemaReport, ArchiveSchemaStatus, ContentArchiveManifestEntry,
    ContentArchiveMigrationAction, ContentManifestRecoveryAction, MmapRecordArchive,
    RecordArchiveMigrationAction, SidecarKind,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveRebuildRoute {
    Ready,
    Migrate,
    Rebuild,
    Recover,
    CannotRecover,
}

impl ArchiveRebuildRoute {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Migrate => "migrate",
            Self::Rebuild => "rebuild",
            Self::Recover => "recover",
            Self::CannotRecover => "cannot-recover",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveRebuildPlanEntry {
    pub kind: &'static str,
    pub path: PathBuf,
    pub status: String,
    pub route: ArchiveRebuildRoute,
    pub source: &'static str,
    pub detail: Option<String>,
}

impl ArchiveRebuildPlanEntry {
    pub fn as_tsv(&self) -> String {
        format!(
            "archive-rebuild-entry\tkind={}\troute={}\tstatus={}\tsource={}\tpath={}\tdetail={}",
            self.kind,
            self.route.as_str(),
            self.status,
            self.source,
            schema::escape_field(&self.path.display().to_string()),
            self.detail
                .as_deref()
                .map(schema::escape_field)
                .unwrap_or("-".to_string())
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveRebuildPlan {
    pub entries: Vec<ArchiveRebuildPlanEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveRebuildInputs {
    pub records_path: PathBuf,
    pub columns_path: PathBuf,
    pub metadata_path: PathBuf,
    pub prefixes_path: PathBuf,
    pub substrings_path: PathBuf,
    pub fuzzy_path: PathBuf,
    pub dictionary_path: PathBuf,
    pub content_path: PathBuf,
    pub manifest_path: PathBuf,
    pub discovered_content_archives: Vec<ContentArchiveManifestEntry>,
}

impl ArchiveRebuildPlan {
    pub fn ready_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.route == ArchiveRebuildRoute::Ready)
            .count()
    }

    pub fn migration_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.route == ArchiveRebuildRoute::Migrate)
            .count()
    }

    pub fn rebuild_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.route == ArchiveRebuildRoute::Rebuild)
            .count()
    }

    pub fn recovery_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.route == ArchiveRebuildRoute::Recover)
            .count()
    }

    pub fn blocked_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.route == ArchiveRebuildRoute::CannotRecover)
            .count()
    }

    pub fn as_tsv_lines(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "archive-rebuild-plan\tentries={}\tready={}\tmigrate={}\trebuild={}\trecover={}\tblocked={}",
            self.entries.len(),
            self.ready_count(),
            self.migration_count(),
            self.rebuild_count(),
            self.recovery_count(),
            self.blocked_count()
        )];
        lines.extend(self.entries.iter().map(ArchiveRebuildPlanEntry::as_tsv));
        lines
    }
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
    rebuild_derived_sidecar_checked(records_path, kind, sidecar_path, backup_dir, || Ok(()))
}

pub fn rebuild_derived_sidecar_checked(
    records_path: impl AsRef<Path>,
    kind: SidecarKind,
    sidecar_path: impl AsRef<Path>,
    backup_dir: impl AsRef<Path>,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<DerivedSidecarRebuild> {
    let records_path = records_path.as_ref();
    let sidecar_path = sidecar_path.as_ref();
    let backup_dir = backup_dir.as_ref();
    check_control()?;
    let before = plan_derived_sidecar_rebuild(records_path, kind, sidecar_path);
    check_control()?;
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

    check_control()?;
    let records = MmapRecordArchive::open_checked(records_path, &mut check_control)?
        .records_checked(&mut check_control)?;
    check_control()?;
    let backup_path = if sidecar_path.try_exists().map_err(|err| {
        GfmError::io(
            sidecar_path,
            format!("derived sidecar existence unavailable: {err}"),
        )
    })? {
        let label = match before.sidecar.status {
            ArchiveSchemaStatus::Legacy => "legacy",
            ArchiveSchemaStatus::Unsupported => "unsupported",
            ArchiveSchemaStatus::Unreadable => "unreadable",
            ArchiveSchemaStatus::Current | ArchiveSchemaStatus::Missing => sidecar_kind_name(kind),
        };
        let backup = schema::backup_archive(sidecar_path, backup_dir, label)?;
        check_control()?;
        Some(backup)
    } else {
        None
    };
    check_control()?;
    write_derived_sidecar_checked(kind, sidecar_path, &records, &mut check_control)?;
    check_control()?;
    let after = inspect_archive_schema(archive_kind_for_sidecar(kind), sidecar_path);
    check_control()?;
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

pub fn plan_archive_rebuilds(inputs: &ArchiveRebuildInputs) -> ArchiveRebuildPlan {
    let records_path = inputs.records_path.as_path();
    let content_path = inputs.content_path.as_path();
    let manifest_path = inputs.manifest_path.as_path();
    let record_plan = plan_record_archive_migration(records_path);
    let content_plan = plan_content_archive_migration(content_path);
    let manifest_plan =
        plan_content_manifest_recovery(manifest_path, &inputs.discovered_content_archives);
    let mut entries = Vec::with_capacity(9);
    entries.push(record_rebuild_entry(record_plan));
    entries.extend([
        derived_rebuild_entry(plan_derived_sidecar_rebuild(
            records_path,
            SidecarKind::Columns,
            &inputs.columns_path,
        )),
        derived_rebuild_entry(plan_derived_sidecar_rebuild(
            records_path,
            SidecarKind::Metadata,
            &inputs.metadata_path,
        )),
        derived_rebuild_entry(plan_derived_sidecar_rebuild(
            records_path,
            SidecarKind::Prefixes,
            &inputs.prefixes_path,
        )),
        derived_rebuild_entry(plan_derived_sidecar_rebuild(
            records_path,
            SidecarKind::Substrings,
            &inputs.substrings_path,
        )),
        derived_rebuild_entry(plan_derived_sidecar_rebuild(
            records_path,
            SidecarKind::Fuzzy,
            &inputs.fuzzy_path,
        )),
        derived_rebuild_entry(plan_derived_sidecar_rebuild(
            records_path,
            SidecarKind::Dictionary,
            &inputs.dictionary_path,
        )),
    ]);
    entries.push(content_rebuild_entry(content_plan));
    entries.push(content_manifest_rebuild_entry(manifest_plan));
    ArchiveRebuildPlan { entries }
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

fn record_rebuild_entry(plan: crate::RecordArchiveMigrationPlan) -> ArchiveRebuildPlanEntry {
    let route = match plan.action {
        RecordArchiveMigrationAction::Ready => ArchiveRebuildRoute::Ready,
        RecordArchiveMigrationAction::Migrate => ArchiveRebuildRoute::Migrate,
        RecordArchiveMigrationAction::CannotMigrate => match plan.before.status {
            ArchiveSchemaStatus::Missing | ArchiveSchemaStatus::Unreadable => {
                ArchiveRebuildRoute::Rebuild
            }
            ArchiveSchemaStatus::Unsupported
            | ArchiveSchemaStatus::Current
            | ArchiveSchemaStatus::Legacy => ArchiveRebuildRoute::CannotRecover,
        },
    };
    ArchiveRebuildPlanEntry {
        kind: ArchiveSchemaKind::Records.as_str(),
        path: plan.before.path,
        status: plan.before.status.as_str().to_string(),
        route,
        source: match route {
            ArchiveRebuildRoute::Ready => "current-records",
            ArchiveRebuildRoute::Migrate => "legacy-records",
            ArchiveRebuildRoute::Rebuild => "filesystem-scan",
            ArchiveRebuildRoute::Recover | ArchiveRebuildRoute::CannotRecover => "operator",
        },
        detail: plan.detail,
    }
}

fn content_rebuild_entry(plan: crate::ContentArchiveMigrationPlan) -> ArchiveRebuildPlanEntry {
    let route = match plan.action {
        ContentArchiveMigrationAction::Ready => ArchiveRebuildRoute::Ready,
        ContentArchiveMigrationAction::Migrate => ArchiveRebuildRoute::Migrate,
        ContentArchiveMigrationAction::CannotMigrate => match plan.before.status {
            ArchiveSchemaStatus::Missing | ArchiveSchemaStatus::Unreadable => {
                ArchiveRebuildRoute::Rebuild
            }
            ArchiveSchemaStatus::Unsupported
            | ArchiveSchemaStatus::Current
            | ArchiveSchemaStatus::Legacy => ArchiveRebuildRoute::CannotRecover,
        },
    };
    ArchiveRebuildPlanEntry {
        kind: ArchiveSchemaKind::Content.as_str(),
        path: plan.before.path,
        status: plan.before.status.as_str().to_string(),
        route,
        source: match route {
            ArchiveRebuildRoute::Ready => "current-content",
            ArchiveRebuildRoute::Migrate => "legacy-content",
            ArchiveRebuildRoute::Rebuild => "extraction-segments",
            ArchiveRebuildRoute::Recover | ArchiveRebuildRoute::CannotRecover => "operator",
        },
        detail: plan.detail,
    }
}

fn derived_rebuild_entry(plan: DerivedSidecarRebuildPlan) -> ArchiveRebuildPlanEntry {
    let route = match plan.action {
        DerivedSidecarRebuildAction::Ready => ArchiveRebuildRoute::Ready,
        DerivedSidecarRebuildAction::Rebuild => ArchiveRebuildRoute::Rebuild,
        DerivedSidecarRebuildAction::CannotRebuild => ArchiveRebuildRoute::CannotRecover,
    };
    ArchiveRebuildPlanEntry {
        kind: sidecar_kind_name(plan.kind),
        path: plan.sidecar.path,
        status: plan.sidecar.status.as_str().to_string(),
        route,
        source: match route {
            ArchiveRebuildRoute::Ready => "current-sidecar",
            ArchiveRebuildRoute::Rebuild => "durable-records",
            ArchiveRebuildRoute::Migrate
            | ArchiveRebuildRoute::Recover
            | ArchiveRebuildRoute::CannotRecover => "operator",
        },
        detail: plan.detail,
    }
}

fn content_manifest_rebuild_entry(
    plan: crate::ContentManifestRecoveryPlan,
) -> ArchiveRebuildPlanEntry {
    let route = match plan.action {
        ContentManifestRecoveryAction::Ready => ArchiveRebuildRoute::Ready,
        ContentManifestRecoveryAction::WriteDiscoveredManifest
        | ContentManifestRecoveryAction::QuarantineManifestAndWriteDiscovered
        | ContentManifestRecoveryAction::PruneInvalidArchives => ArchiveRebuildRoute::Recover,
        ContentManifestRecoveryAction::CannotRecover => ArchiveRebuildRoute::CannotRecover,
    };
    ArchiveRebuildPlanEntry {
        kind: ArchiveSchemaKind::ContentManifest.as_str(),
        path: plan.manifest_path,
        status: content_manifest_recovery_action_name(plan.action).to_string(),
        route,
        source: match route {
            ArchiveRebuildRoute::Ready => "current-manifest",
            ArchiveRebuildRoute::Recover => "validated-content-archives",
            ArchiveRebuildRoute::Migrate
            | ArchiveRebuildRoute::Rebuild
            | ArchiveRebuildRoute::CannotRecover => "operator",
        },
        detail: plan.detail,
    }
}

fn archive_kind_for_sidecar(kind: SidecarKind) -> ArchiveSchemaKind {
    match kind {
        SidecarKind::Columns => ArchiveSchemaKind::Columns,
        SidecarKind::Metadata => ArchiveSchemaKind::Metadata,
        SidecarKind::Prefixes => ArchiveSchemaKind::Prefixes,
        SidecarKind::Substrings => ArchiveSchemaKind::Substrings,
        SidecarKind::Fuzzy => ArchiveSchemaKind::Fuzzy,
        SidecarKind::Dictionary => ArchiveSchemaKind::Dictionary,
    }
}

fn write_derived_sidecar_checked(
    kind: SidecarKind,
    path: &Path,
    records: &[FileRecord],
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    match kind {
        SidecarKind::Columns => write_record_columns(path, records),
        SidecarKind::Metadata => {
            write_metadata_postings(path, &metadata_postings_from_records(records))
        }
        SidecarKind::Prefixes => {
            let postings = prefix_postings_from_records(records);
            write_prefix_postings_checked(path, &postings, &mut check_control)
        }
        SidecarKind::Substrings => {
            let postings = substring_postings_from_records(records);
            write_substring_postings_checked(path, &postings, &mut check_control)
        }
        SidecarKind::Fuzzy => {
            let postings = fuzzy_postings_from_records(records);
            write_fuzzy_postings_checked(path, &postings, &mut check_control)
        }
        SidecarKind::Dictionary => write_dictionary(path, &dictionary_terms_from_records(records)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        read_dictionary, read_metadata_postings, write_content_postings, write_records,
        ContentMergeTier, MmapFuzzyArchive, MmapPrefixArchive, MmapRecordColumns,
        MmapSubstringArchive,
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
            (SidecarKind::Substrings, "records.gfmsubstr"),
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
        assert!(!MmapSubstringArchive::open(dir.join("records.gfmsubstr"))
            .unwrap()
            .ids_for("sta")
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

    #[test]
    fn derived_sidecar_rebuild_surfaces_sidecar_path_probe_failures() {
        let dir = temp_dir("gfm-derived-sidecar-rebuild-probe");
        let records = dir.join("records.gfmidx");
        let prefixes = dir.join("derived-sidecar-unavailable".repeat(64));
        let backup = dir.join("backup");
        write_records(&records, &[record()]).unwrap();

        let plan = plan_derived_sidecar_rebuild(&records, SidecarKind::Prefixes, &prefixes);
        assert_eq!(plan.action, DerivedSidecarRebuildAction::Rebuild);
        assert_eq!(plan.sidecar.status, ArchiveSchemaStatus::Unreadable);
        assert!(plan
            .sidecar
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("archive schema existence unavailable")));

        let err = rebuild_derived_sidecar(&records, SidecarKind::Prefixes, &prefixes, &backup)
            .unwrap_err();

        assert!(err
            .to_string()
            .contains("derived sidecar existence unavailable"));
        assert!(!backup.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn checked_derived_sidecar_rebuild_stops_before_publication() {
        let dir = temp_dir("gfm-derived-sidecar-rebuild-cancel");
        let records = dir.join("records.gfmidx");
        let prefixes = dir.join("records.gfmprefix");
        let backup = dir.join("backup");
        write_records(&records, &[record()]).unwrap();
        let mut checks = 0_u32;

        let result = rebuild_derived_sidecar_checked(
            &records,
            SidecarKind::Prefixes,
            &prefixes,
            &backup,
            || {
                checks += 1;
                if checks > 4 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(!prefixes.exists());
        assert!(!backup.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn checked_derived_sidecar_rebuild_cancels_during_record_open_before_sidecar_probe() {
        let dir = temp_dir("gfm-derived-sidecar-rebuild-record-open-cancel");
        let records = dir.join("records.gfmidx");
        let sidecar = dir.join("x".repeat(512));
        let backup = dir.join("backup");
        write_records(&records, &[record()]).unwrap();
        let mut checks = 0_u32;

        let result = rebuild_derived_sidecar_checked(
            &records,
            SidecarKind::Prefixes,
            &sidecar,
            &backup,
            || {
                checks += 1;
                if checks >= 5 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert_eq!(checks, 5);
        assert!(!backup.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn plans_rebuild_routes_across_all_archive_types() {
        let dir = temp_dir("gfm-archive-rebuild-plan-all");
        let records = dir.join("records.gfmidx");
        let columns = dir.join("records.gfmcols");
        let metadata = dir.join("records.gfmmeta");
        let prefixes = dir.join("records.gfmprefix");
        let substrings = dir.join("records.gfmsubstr");
        let fuzzy = dir.join("records.gfmfuzzy");
        let dictionary = dir.join("records.gfmdict");
        let content = dir.join("content.gfmcontent");
        let manifest = dir.join("content.gfmmanifest");
        write_records(&records, &[record()]).unwrap();
        write_content_postings(&content, &[]).unwrap();

        let plan = plan_archive_rebuilds(&ArchiveRebuildInputs {
            records_path: records,
            columns_path: columns,
            metadata_path: metadata,
            prefixes_path: prefixes,
            substrings_path: substrings,
            fuzzy_path: fuzzy,
            dictionary_path: dictionary,
            content_path: content.clone(),
            manifest_path: manifest,
            discovered_content_archives: vec![ContentArchiveManifestEntry {
                tier: ContentMergeTier::Hot,
                path: content.clone(),
            }],
        });

        assert_eq!(plan.entries.len(), 9);
        assert_eq!(plan.ready_count(), 2);
        assert_eq!(plan.rebuild_count(), 6);
        assert_eq!(plan.recovery_count(), 1);
        assert_eq!(plan.blocked_count(), 0);
        assert!(plan
            .entries
            .iter()
            .any(|entry| entry.kind == "records" && entry.route == ArchiveRebuildRoute::Ready));
        assert!(plan.entries.iter().any(|entry| entry.kind == "columns"
            && entry.route == ArchiveRebuildRoute::Rebuild
            && entry.source == "durable-records"));
        assert!(plan.entries.iter().any(|entry| entry.kind == "substrings"
            && entry.route == ArchiveRebuildRoute::Rebuild
            && entry.source == "durable-records"));
        assert!(plan
            .entries
            .iter()
            .any(|entry| entry.kind == "content-manifest"
                && entry.route == ArchiveRebuildRoute::Recover
                && entry.source == "validated-content-archives"));
        assert!(plan
            .as_tsv_lines()
            .first()
            .unwrap()
            .contains("entries=9\tready=2\tmigrate=0\trebuild=6\trecover=1\tblocked=0"));

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn archive_rebuild_plan_blocks_sidecars_without_records() {
        let dir = temp_dir("gfm-archive-rebuild-plan-blocked");
        let records = dir.join("missing.gfmidx");
        let content = dir.join("content.gfmcontent");
        let manifest = dir.join("content.gfmmanifest");

        let plan = plan_archive_rebuilds(&ArchiveRebuildInputs {
            records_path: records,
            columns_path: dir.join("records.gfmcols"),
            metadata_path: dir.join("records.gfmmeta"),
            prefixes_path: dir.join("records.gfmprefix"),
            substrings_path: dir.join("records.gfmsubstr"),
            fuzzy_path: dir.join("records.gfmfuzzy"),
            dictionary_path: dir.join("records.gfmdict"),
            content_path: content,
            manifest_path: manifest,
            discovered_content_archives: Vec::new(),
        });

        assert_eq!(plan.entries.len(), 9);
        assert_eq!(plan.rebuild_count(), 2);
        assert_eq!(plan.blocked_count(), 7);
        assert!(plan.entries.iter().any(|entry| entry.kind == "records"
            && entry.route == ArchiveRebuildRoute::Rebuild
            && entry.source == "filesystem-scan"));
        assert!(plan
            .entries
            .iter()
            .any(|entry| entry.kind == "prefixes"
                && entry.route == ArchiveRebuildRoute::CannotRecover));
        assert!(plan
            .entries
            .iter()
            .any(|entry| entry.kind == "substrings"
                && entry.route == ArchiveRebuildRoute::CannotRecover));
        assert!(plan
            .entries
            .iter()
            .any(|entry| entry.kind == "content-manifest"
                && entry.route == ArchiveRebuildRoute::CannotRecover));

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
