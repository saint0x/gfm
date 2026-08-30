use crate::{
    dictionary_terms_from_records, fuzzy_postings_from_records, metadata_postings_from_records,
    prefix_postings_from_records, substring_postings_from_records, write_dictionary,
    write_fuzzy_postings, write_metadata_postings, write_prefix_postings, write_record_columns,
    write_substring_postings, MmapDictionary, MmapFuzzyArchive, MmapMetadataArchive,
    MmapPrefixArchive, MmapRecordArchive, MmapRecordColumns, MmapSubstringArchive,
};
use gfm_types::{FileRecord, GfmError, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SidecarKind {
    Columns,
    Metadata,
    Prefixes,
    Substrings,
    Fuzzy,
    Dictionary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarRecoveryAction {
    Ready,
    Rebuild,
    CannotRecover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarRecoveryReason {
    Healthy,
    UnreadableRecords,
    MissingSidecar,
    UnreadableSidecar,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SidecarPaths {
    pub columns: Option<PathBuf>,
    pub metadata: Option<PathBuf>,
    pub prefixes: Option<PathBuf>,
    pub substrings: Option<PathBuf>,
    pub fuzzy: Option<PathBuf>,
    pub dictionary: Option<PathBuf>,
}

impl SidecarPaths {
    pub fn iter(&self) -> impl Iterator<Item = (SidecarKind, &PathBuf)> {
        [
            (SidecarKind::Columns, self.columns.as_ref()),
            (SidecarKind::Metadata, self.metadata.as_ref()),
            (SidecarKind::Prefixes, self.prefixes.as_ref()),
            (SidecarKind::Substrings, self.substrings.as_ref()),
            (SidecarKind::Fuzzy, self.fuzzy.as_ref()),
            (SidecarKind::Dictionary, self.dictionary.as_ref()),
        ]
        .into_iter()
        .filter_map(|(kind, path)| path.map(|path| (kind, path)))
    }

    pub fn is_empty(&self) -> bool {
        self.iter().next().is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarHealth {
    pub kind: SidecarKind,
    pub path: PathBuf,
    pub valid: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarRecoveryPlan {
    pub action: SidecarRecoveryAction,
    pub reason: SidecarRecoveryReason,
    pub records_path: PathBuf,
    pub record_count: Option<usize>,
    pub valid_sidecars: Vec<SidecarHealth>,
    pub invalid_sidecars: Vec<SidecarHealth>,
    pub detail: Option<String>,
}

impl SidecarRecoveryPlan {
    pub fn ready(&self) -> bool {
        self.action == SidecarRecoveryAction::Ready
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "sidecar-recovery-plan\taction={}\treason={}\trecords={}\trecord-count={}\tvalid={}\tinvalid={}\tdetail={}",
            sidecar_recovery_action_name(self.action),
            sidecar_recovery_reason_name(self.reason),
            self.records_path.display(),
            self.record_count
                .map(|count| count.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.valid_sidecars.len(),
            self.invalid_sidecars.len(),
            self.detail.as_deref().unwrap_or("-")
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarRecovery {
    pub before: SidecarRecoveryPlan,
    pub after: SidecarRecoveryPlan,
    pub rebuilt_sidecars: Vec<SidecarKind>,
    pub quarantined_sidecars: Vec<PathBuf>,
}

pub fn sidecar_kind_name(kind: SidecarKind) -> &'static str {
    match kind {
        SidecarKind::Columns => "columns",
        SidecarKind::Metadata => "metadata",
        SidecarKind::Prefixes => "prefixes",
        SidecarKind::Substrings => "substrings",
        SidecarKind::Fuzzy => "fuzzy",
        SidecarKind::Dictionary => "dictionary",
    }
}

pub fn sidecar_recovery_action_name(action: SidecarRecoveryAction) -> &'static str {
    match action {
        SidecarRecoveryAction::Ready => "ready",
        SidecarRecoveryAction::Rebuild => "rebuild",
        SidecarRecoveryAction::CannotRecover => "cannot-recover",
    }
}

pub fn sidecar_recovery_reason_name(reason: SidecarRecoveryReason) -> &'static str {
    match reason {
        SidecarRecoveryReason::Healthy => "healthy",
        SidecarRecoveryReason::UnreadableRecords => "unreadable-records",
        SidecarRecoveryReason::MissingSidecar => "missing-sidecar",
        SidecarRecoveryReason::UnreadableSidecar => "unreadable-sidecar",
    }
}

pub fn plan_sidecar_recovery(
    records_path: impl AsRef<Path>,
    sidecars: &SidecarPaths,
) -> SidecarRecoveryPlan {
    plan_sidecar_recovery_checked(records_path, sidecars, || Ok(()))
        .expect("infallible sidecar recovery planning cancellation")
}

pub fn plan_sidecar_recovery_checked(
    records_path: impl AsRef<Path>,
    sidecars: &SidecarPaths,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<SidecarRecoveryPlan> {
    let records_path = records_path.as_ref().to_path_buf();
    check_control()?;
    let records = match MmapRecordArchive::open_checked(&records_path, &mut check_control)
        .and_then(|archive| archive.records_checked(&mut check_control))
    {
        Ok(records) => {
            check_control()?;
            records
        }
        Err(err) => {
            return Ok(SidecarRecoveryPlan {
                action: SidecarRecoveryAction::CannotRecover,
                reason: SidecarRecoveryReason::UnreadableRecords,
                records_path,
                record_count: None,
                valid_sidecars: Vec::new(),
                invalid_sidecars: Vec::new(),
                detail: Some(err.to_string()),
            });
        }
    };

    let (valid_sidecars, invalid_sidecars) =
        classify_sidecars_checked(sidecars, &mut check_control)?;
    check_control()?;
    if invalid_sidecars.is_empty() {
        return Ok(SidecarRecoveryPlan {
            action: SidecarRecoveryAction::Ready,
            reason: SidecarRecoveryReason::Healthy,
            records_path,
            record_count: Some(records.len()),
            valid_sidecars,
            invalid_sidecars,
            detail: None,
        });
    }

    Ok(SidecarRecoveryPlan {
        action: SidecarRecoveryAction::Rebuild,
        reason: invalid_sidecar_reason(&invalid_sidecars),
        records_path,
        record_count: Some(records.len()),
        valid_sidecars,
        invalid_sidecars,
        detail: None,
    })
}

pub fn recover_sidecars(
    records_path: impl AsRef<Path>,
    sidecars: &SidecarPaths,
    quarantine_dir: impl AsRef<Path>,
) -> Result<SidecarRecovery> {
    recover_sidecars_checked(records_path, sidecars, quarantine_dir, || Ok(()))
}

pub fn recover_sidecars_checked(
    records_path: impl AsRef<Path>,
    sidecars: &SidecarPaths,
    quarantine_dir: impl AsRef<Path>,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<SidecarRecovery> {
    let records_path = records_path.as_ref();
    let quarantine_dir = quarantine_dir.as_ref();
    check_control()?;
    let before = plan_sidecar_recovery_checked(records_path, sidecars, &mut check_control)?;
    check_control()?;
    if before.action == SidecarRecoveryAction::CannotRecover {
        return Err(GfmError::Format(format!(
            "{} cannot rebuild sidecars: {}",
            records_path.display(),
            before.detail.as_deref().unwrap_or("records are unreadable")
        )));
    }

    let mut rebuilt_sidecars = Vec::new();
    let mut quarantined_sidecars = Vec::new();
    if before.action == SidecarRecoveryAction::Rebuild {
        check_control()?;
        let records = MmapRecordArchive::open_checked(records_path, &mut check_control)?
            .records_checked(&mut check_control)?;
        check_control()?;
        for health in &before.invalid_sidecars {
            check_control()?;
            if health.path.try_exists().map_err(|err| {
                GfmError::io(
                    &health.path,
                    format!("sidecar archive existence unavailable: {err}"),
                )
            })? {
                quarantined_sidecars.push(quarantine_sidecar(&health.path, quarantine_dir)?);
                check_control()?;
            }
            rebuild_sidecar(health.kind, &health.path, &records)?;
            check_control()?;
            rebuilt_sidecars.push(health.kind);
        }
    }

    check_control()?;
    let after = plan_sidecar_recovery_checked(records_path, sidecars, check_control)?;
    Ok(SidecarRecovery {
        before,
        after,
        rebuilt_sidecars,
        quarantined_sidecars,
    })
}

fn classify_sidecars_checked(
    sidecars: &SidecarPaths,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<(Vec<SidecarHealth>, Vec<SidecarHealth>)> {
    let mut valid = Vec::new();
    let mut invalid = Vec::new();
    for (kind, path) in sidecars.iter() {
        check_control()?;
        let detail = match path.try_exists() {
            Ok(true) => match open_sidecar_checked(kind, path, &mut check_control) {
                Ok(()) => None,
                Err(GfmError::Cancelled) => return Err(GfmError::Cancelled),
                Err(GfmError::Paused) => return Err(GfmError::Paused),
                Err(err) => Some(err.to_string()),
            },
            Ok(false) => Some("missing sidecar archive".to_string()),
            Err(err) => Some(format!("sidecar archive existence unavailable: {err}")),
        };
        check_control()?;
        let health = SidecarHealth {
            kind,
            path: path.clone(),
            valid: detail.is_none(),
            detail,
        };
        if health.valid {
            valid.push(health);
        } else {
            invalid.push(health);
        }
    }
    Ok((valid, invalid))
}

fn open_sidecar_checked(
    kind: SidecarKind,
    path: &Path,
    check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    match kind {
        SidecarKind::Columns => MmapRecordColumns::open_checked(path, check_control).map(|_| ()),
        SidecarKind::Metadata => MmapMetadataArchive::open_checked(path, check_control).map(|_| ()),
        SidecarKind::Prefixes => MmapPrefixArchive::open_checked(path, check_control).map(|_| ()),
        SidecarKind::Substrings => {
            MmapSubstringArchive::open_checked(path, check_control).map(|_| ())
        }
        SidecarKind::Fuzzy => MmapFuzzyArchive::open_checked(path, check_control).map(|_| ()),
        SidecarKind::Dictionary => MmapDictionary::open_checked(path, check_control).map(|_| ()),
    }
}

fn rebuild_sidecar(kind: SidecarKind, path: &Path, records: &[FileRecord]) -> Result<()> {
    match kind {
        SidecarKind::Columns => write_record_columns(path, records),
        SidecarKind::Metadata => {
            write_metadata_postings(path, &metadata_postings_from_records(records))
        }
        SidecarKind::Prefixes => {
            write_prefix_postings(path, &prefix_postings_from_records(records))
        }
        SidecarKind::Substrings => {
            write_substring_postings(path, &substring_postings_from_records(records))
        }
        SidecarKind::Fuzzy => write_fuzzy_postings(path, &fuzzy_postings_from_records(records)),
        SidecarKind::Dictionary => write_dictionary(path, &dictionary_terms_from_records(records)),
    }
}

fn invalid_sidecar_reason(invalid_sidecars: &[SidecarHealth]) -> SidecarRecoveryReason {
    if invalid_sidecars
        .iter()
        .any(|sidecar| sidecar.detail.as_deref() == Some("missing sidecar archive"))
    {
        SidecarRecoveryReason::MissingSidecar
    } else {
        SidecarRecoveryReason::UnreadableSidecar
    }
}

fn quarantine_sidecar(path: &Path, quarantine_dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(quarantine_dir).map_err(|err| GfmError::io(quarantine_dir, err))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("sidecar.archive");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let quarantine_path =
        quarantine_dir.join(format!("{name}.corrupt.{}.{}", std::process::id(), nanos));
    fs::rename(path, &quarantine_path).map_err(|err| GfmError::io(path, err))?;
    Ok(quarantine_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{read_dictionary, write_records, MmapSubstringArchive};
    use gfm_types::{FileId, FileKind, VolumeId};

    #[test]
    fn sidecar_recovery_rebuilds_missing_sidecars_from_records() {
        let dir = temp_dir("gfm-sidecar-recovery-missing");
        let records = dir.join("records.gfmidx");
        let columns = dir.join("records.gfmcols");
        let prefixes = dir.join("records.gfmprefix");
        let substrings = dir.join("records.gfmsubstr");
        fs::create_dir_all(&dir).unwrap();
        write_records(&records, &[record("ProjectPlan.md")]).unwrap();
        let paths = SidecarPaths {
            columns: Some(columns.clone()),
            prefixes: Some(prefixes.clone()),
            substrings: Some(substrings.clone()),
            ..SidecarPaths::default()
        };

        let plan = plan_sidecar_recovery(&records, &paths);
        assert_eq!(plan.action, SidecarRecoveryAction::Rebuild);
        assert_eq!(plan.reason, SidecarRecoveryReason::MissingSidecar);
        assert_eq!(plan.invalid_sidecars.len(), 3);

        let recovery = recover_sidecars(&records, &paths, dir.join("quarantine")).unwrap();

        assert_eq!(recovery.rebuilt_sidecars.len(), 3);
        assert!(recovery.after.ready());
        assert!(MmapRecordColumns::open(columns).unwrap().is_checksummed());
        assert!(MmapPrefixArchive::open(prefixes).unwrap().is_checksummed());
        assert!(MmapSubstringArchive::open(substrings)
            .unwrap()
            .is_checksummed());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sidecar_recovery_quarantines_corrupt_sidecar_before_rebuild() {
        let dir = temp_dir("gfm-sidecar-recovery-corrupt");
        let records = dir.join("records.gfmidx");
        let dictionary = dir.join("records.gfmdict");
        let quarantine = dir.join("quarantine");
        fs::create_dir_all(&dir).unwrap();
        write_records(&records, &[record("ProjectPlan.md")]).unwrap();
        fs::write(&dictionary, "not-a-dictionary").unwrap();
        let paths = SidecarPaths {
            dictionary: Some(dictionary.clone()),
            ..SidecarPaths::default()
        };

        let plan = plan_sidecar_recovery(&records, &paths);
        assert_eq!(plan.action, SidecarRecoveryAction::Rebuild);
        assert_eq!(plan.reason, SidecarRecoveryReason::UnreadableSidecar);

        let recovery = recover_sidecars(&records, &paths, &quarantine).unwrap();

        assert_eq!(recovery.rebuilt_sidecars, vec![SidecarKind::Dictionary]);
        assert!(recovery
            .quarantined_sidecars
            .iter()
            .any(|path| path.exists()));
        assert!(recovery.after.ready());
        assert!(read_dictionary(&dictionary)
            .unwrap()
            .iter()
            .any(|term| term.contains("projectplan")));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sidecar_recovery_surfaces_sidecar_path_probe_failures() {
        let dir = temp_dir("gfm-sidecar-recovery-probe");
        let records = dir.join("records.gfmidx");
        let dictionary = dir.join("sidecar-unavailable".repeat(64));
        let quarantine = dir.join("quarantine");
        fs::create_dir_all(&dir).unwrap();
        write_records(&records, &[record("ProjectPlan.md")]).unwrap();
        let paths = SidecarPaths {
            dictionary: Some(dictionary.clone()),
            ..SidecarPaths::default()
        };

        let plan = plan_sidecar_recovery(&records, &paths);
        assert_eq!(plan.action, SidecarRecoveryAction::Rebuild);
        assert_eq!(plan.reason, SidecarRecoveryReason::UnreadableSidecar);
        assert!(plan.invalid_sidecars[0]
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("sidecar archive existence unavailable")));

        let err = recover_sidecars(&records, &paths, &quarantine).unwrap_err();

        assert!(err
            .to_string()
            .contains("sidecar archive existence unavailable"));
        assert!(!quarantine.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn checked_sidecar_recovery_plan_honors_pre_cancelled_control_before_records_open() {
        let dir = temp_dir("gfm-sidecar-recovery-plan-cancel");
        let records = dir.join("records.gfmidx");
        fs::create_dir_all(&dir).unwrap();

        let result = plan_sidecar_recovery_checked(&records, &SidecarPaths::default(), || {
            Err(GfmError::Cancelled)
        });

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(!records.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn checked_sidecar_validation_honors_pre_cancelled_control_before_archive_open() {
        let dir = temp_dir("gfm-sidecar-validation-open-cancel");
        let dictionary = dir.join("records.gfmdict");
        fs::create_dir_all(&dir).unwrap();

        let result = open_sidecar_checked(SidecarKind::Dictionary, &dictionary, || {
            Err(GfmError::Cancelled)
        });

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(!dictionary.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn checked_sidecar_recovery_stops_before_rebuilding_missing_sidecar() {
        let dir = temp_dir("gfm-sidecar-recovery-cancel");
        let records = dir.join("records.gfmidx");
        let prefixes = dir.join("records.gfmprefix");
        fs::create_dir_all(&dir).unwrap();
        write_records(&records, &[record("ProjectPlan.md")]).unwrap();
        let paths = SidecarPaths {
            prefixes: Some(prefixes.clone()),
            ..SidecarPaths::default()
        };
        let mut checks = 0_u32;

        let result = recover_sidecars_checked(&records, &paths, dir.join("quarantine"), || {
            checks += 1;
            if checks > 3 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(!prefixes.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    fn record(name: &str) -> FileRecord {
        FileRecord {
            id: FileId::new(VolumeId(1), 1),
            parent: None,
            path: PathBuf::from(format!("/tmp/{name}")),
            name: name.to_string(),
            kind: FileKind::File,
            len: 4,
            mode: 0,
            owner: 0,
            group: 0,
            xattrs_digest: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: vec!["Important".to_string()],
            finder_comment: Some("Launch notes".to_string()),
        }
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{}-{}",
            prefix,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
}
