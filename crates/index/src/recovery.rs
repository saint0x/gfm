use crate::{IndexSnapshot, IndexVolumeState};
use gfm_store::read_records;
use gfm_types::{GfmError, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistentIndexAction {
    Ready,
    RebuildState,
    RebuildRecordsAndState,
    MigrateState,
    QuarantineRecordsAndRebuild,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistentIndexReason {
    Healthy,
    MissingRecords,
    MissingState,
    UnreadableRecords,
    UnreadableState,
    UnsupportedStateSchema,
    RootMismatch,
    RecordsPathMismatch,
    RecordCountMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistentIndexPlan {
    pub action: PersistentIndexAction,
    pub reason: PersistentIndexReason,
    pub root: PathBuf,
    pub records_path: PathBuf,
    pub state_path: PathBuf,
    pub record_count: Option<usize>,
    pub state_record_count: Option<usize>,
    pub state_schema_version: Option<u32>,
    pub detail: Option<String>,
}

impl PersistentIndexPlan {
    pub fn ready(&self) -> bool {
        self.action == PersistentIndexAction::Ready
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "persistent-index-plan\taction={}\treason={}\troot={}\trecords={}\tstate={}\trecord-count={}\tstate-record-count={}\tstate-schema={}\tdetail={}",
            persistent_index_action_name(self.action),
            persistent_index_reason_name(self.reason),
            self.root.display(),
            self.records_path.display(),
            self.state_path.display(),
            optional_usize(self.record_count),
            optional_usize(self.state_record_count),
            optional_u32(self.state_schema_version),
            self.detail.as_deref().unwrap_or("-")
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistentIndexRecovery {
    pub before: PersistentIndexPlan,
    pub after: PersistentIndexPlan,
    pub rebuilt_records: bool,
    pub rebuilt_state: bool,
    pub quarantined_records_path: Option<PathBuf>,
}

pub fn persistent_index_action_name(action: PersistentIndexAction) -> &'static str {
    match action {
        PersistentIndexAction::Ready => "ready",
        PersistentIndexAction::RebuildState => "rebuild-state",
        PersistentIndexAction::RebuildRecordsAndState => "rebuild-records-and-state",
        PersistentIndexAction::MigrateState => "migrate-state",
        PersistentIndexAction::QuarantineRecordsAndRebuild => "quarantine-records-and-rebuild",
    }
}

pub fn persistent_index_reason_name(reason: PersistentIndexReason) -> &'static str {
    match reason {
        PersistentIndexReason::Healthy => "healthy",
        PersistentIndexReason::MissingRecords => "missing-records",
        PersistentIndexReason::MissingState => "missing-state",
        PersistentIndexReason::UnreadableRecords => "unreadable-records",
        PersistentIndexReason::UnreadableState => "unreadable-state",
        PersistentIndexReason::UnsupportedStateSchema => "unsupported-state-schema",
        PersistentIndexReason::RootMismatch => "root-mismatch",
        PersistentIndexReason::RecordsPathMismatch => "records-path-mismatch",
        PersistentIndexReason::RecordCountMismatch => "record-count-mismatch",
    }
}

pub fn plan_persistent_index_recovery(
    root: impl AsRef<Path>,
    records_path: impl AsRef<Path>,
    state_path: impl AsRef<Path>,
) -> PersistentIndexPlan {
    let root = root.as_ref().to_path_buf();
    let records_path = records_path.as_ref().to_path_buf();
    let state_path = state_path.as_ref().to_path_buf();

    let records_exist = match records_path.try_exists() {
        Ok(exists) => exists,
        Err(err) => {
            return PersistentIndexPlan {
                action: PersistentIndexAction::QuarantineRecordsAndRebuild,
                reason: PersistentIndexReason::UnreadableRecords,
                root,
                records_path,
                state_path,
                record_count: None,
                state_record_count: None,
                state_schema_version: None,
                detail: Some(format!("record archive existence unavailable: {err}")),
            }
        }
    };
    if !records_exist {
        return PersistentIndexPlan {
            action: PersistentIndexAction::RebuildRecordsAndState,
            reason: PersistentIndexReason::MissingRecords,
            root,
            records_path,
            state_path,
            record_count: None,
            state_record_count: None,
            state_schema_version: None,
            detail: None,
        };
    }

    let records = match read_records(&records_path) {
        Ok(records) => records,
        Err(err) => {
            return PersistentIndexPlan {
                action: PersistentIndexAction::QuarantineRecordsAndRebuild,
                reason: PersistentIndexReason::UnreadableRecords,
                root,
                records_path,
                state_path,
                record_count: None,
                state_record_count: None,
                state_schema_version: None,
                detail: Some(err.to_string()),
            }
        }
    };
    let record_count = Some(records.len());

    let state_exists = match state_path.try_exists() {
        Ok(exists) => exists,
        Err(err) => {
            return PersistentIndexPlan {
                action: PersistentIndexAction::RebuildState,
                reason: PersistentIndexReason::UnreadableState,
                root,
                records_path,
                state_path,
                record_count,
                state_record_count: None,
                state_schema_version: None,
                detail: Some(format!("index state existence unavailable: {err}")),
            }
        }
    };
    if !state_exists {
        return PersistentIndexPlan {
            action: PersistentIndexAction::RebuildState,
            reason: PersistentIndexReason::MissingState,
            root,
            records_path,
            state_path,
            record_count,
            state_record_count: None,
            state_schema_version: None,
            detail: None,
        };
    }

    let state = match IndexVolumeState::read(&state_path) {
        Ok(state) => state,
        Err(err) => {
            let detail = err.to_string();
            let unsupported_schema = detail.contains("unsupported index state schema version");
            let state_schema_version = read_state_schema_version(&state_path);
            return PersistentIndexPlan {
                action: if unsupported_schema {
                    PersistentIndexAction::MigrateState
                } else {
                    PersistentIndexAction::RebuildState
                },
                reason: if unsupported_schema {
                    PersistentIndexReason::UnsupportedStateSchema
                } else {
                    PersistentIndexReason::UnreadableState
                },
                root,
                records_path,
                state_path,
                record_count,
                state_record_count: None,
                state_schema_version,
                detail: Some(detail),
            };
        }
    };

    if state.root != root {
        let detail = format!(
            "state root {} does not match requested root {}",
            state.root.display(),
            root.display()
        );
        return state_rebuild_plan(
            root,
            records_path,
            state_path,
            record_count,
            &state,
            PersistentIndexReason::RootMismatch,
            Some(detail),
        );
    }

    if state.records_path != records_path {
        let detail = format!(
            "state records path {} does not match requested records path {}",
            state.records_path.display(),
            records_path.display()
        );
        return state_rebuild_plan(
            root,
            records_path,
            state_path,
            record_count,
            &state,
            PersistentIndexReason::RecordsPathMismatch,
            Some(detail),
        );
    }

    if state.record_count != records.len() {
        return state_rebuild_plan(
            root,
            records_path,
            state_path,
            record_count,
            &state,
            PersistentIndexReason::RecordCountMismatch,
            Some(format!(
                "state record count {} does not match archive count {}",
                state.record_count,
                records.len()
            )),
        );
    }

    PersistentIndexPlan {
        action: PersistentIndexAction::Ready,
        reason: PersistentIndexReason::Healthy,
        root,
        records_path,
        state_path,
        record_count,
        state_record_count: Some(state.record_count),
        state_schema_version: Some(state.schema_version),
        detail: None,
    }
}

fn state_rebuild_plan(
    root: PathBuf,
    records_path: PathBuf,
    state_path: PathBuf,
    record_count: Option<usize>,
    state: &IndexVolumeState,
    reason: PersistentIndexReason,
    detail: Option<String>,
) -> PersistentIndexPlan {
    PersistentIndexPlan {
        action: PersistentIndexAction::RebuildState,
        reason,
        root,
        records_path,
        state_path,
        record_count,
        state_record_count: Some(state.record_count),
        state_schema_version: Some(state.schema_version),
        detail,
    }
}

pub(crate) fn recover_persistent_index_checked<F>(
    root: impl AsRef<Path>,
    records_path: impl AsRef<Path>,
    state_path: impl AsRef<Path>,
    quarantine_dir: impl AsRef<Path>,
    rebuild_records: F,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<PersistentIndexRecovery>
where
    F: FnOnce() -> Result<IndexVolumeState>,
{
    let root = root.as_ref();
    let records_path = records_path.as_ref();
    let state_path = state_path.as_ref();
    let quarantine_dir = quarantine_dir.as_ref();
    check_control()?;
    let before = plan_persistent_index_recovery(root, records_path, state_path);
    check_control()?;
    let mut rebuilt_records = false;
    let mut rebuilt_state = false;
    let mut quarantined_records_path = None;

    match before.action {
        PersistentIndexAction::Ready => {}
        PersistentIndexAction::RebuildState | PersistentIndexAction::MigrateState => {
            check_control()?;
            write_state_from_records(root, records_path, state_path)?;
            check_control()?;
            rebuilt_state = true;
        }
        PersistentIndexAction::RebuildRecordsAndState => {
            check_control()?;
            rebuild_records()?;
            check_control()?;
            rebuilt_records = true;
            rebuilt_state = true;
        }
        PersistentIndexAction::QuarantineRecordsAndRebuild => {
            check_control()?;
            let quarantine_path = quarantine_records(records_path, quarantine_dir)?;
            check_control()?;
            quarantined_records_path = Some(quarantine_path);
            rebuild_records()?;
            check_control()?;
            rebuilt_records = true;
            rebuilt_state = true;
        }
    }

    check_control()?;
    let after = plan_persistent_index_recovery(root, records_path, state_path);
    check_control()?;
    Ok(PersistentIndexRecovery {
        before,
        after,
        rebuilt_records,
        rebuilt_state,
        quarantined_records_path,
    })
}

fn write_state_from_records(root: &Path, records_path: &Path, state_path: &Path) -> Result<()> {
    let records = read_records(records_path)?;
    let snapshot = IndexSnapshot {
        root: root.to_path_buf(),
        records,
        inaccessible: Vec::new(),
    };
    let previous = IndexVolumeState::read(state_path).ok();
    let state = snapshot.volume_state(records_path.to_path_buf(), previous.as_ref())?;
    state.write(state_path)
}

fn quarantine_records(records_path: &Path, quarantine_dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(quarantine_dir).map_err(|err| GfmError::io(quarantine_dir, err))?;
    let name = records_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("records.gfmidx");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let quarantine_path =
        quarantine_dir.join(format!("{name}.corrupt.{}.{}", std::process::id(), nanos));
    fs::rename(records_path, &quarantine_path).map_err(|err| GfmError::io(records_path, err))?;
    Ok(quarantine_path)
}

fn read_state_schema_version(path: &Path) -> Option<u32> {
    let text = fs::read_to_string(path).ok()?;
    text.lines().find_map(|line| {
        let value = line.strip_prefix("schema_version\t")?;
        value.parse().ok()
    })
}

fn optional_usize(value: Option<usize>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn optional_u32(value: Option<u32>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}
