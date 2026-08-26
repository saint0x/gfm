mod access;
mod conflict;
mod control;
mod journal;
mod plan;
mod preserve;
mod progress;
mod recovery;
mod trashmeta;
mod verify;
mod volume;

use access::destination_probe_path;
pub use access::{
    OperationAccessAction, OperationAccessDecision, OperationAccessGate,
    OperationAccessRequirement, OperationAccessRole,
};
pub use conflict::{
    ConflictPolicy, OperationBatchOutcome, OperationBatchReport, OperationConflictPlan,
};
pub use control::{OperationCancellation, OperationPause};
use journal::{append_journal, now_nanos, operation_status_from_error};
pub use journal::{read_journal, JournalEntry, OperationStatus};
pub use plan::plan_operation;
use plan::plan_operation_checked;
#[cfg(all(test, target_os = "macos"))]
use preserve::acl_copy_unsupported;
#[cfg(all(test, target_vendor = "apple"))]
use preserve::file_flag_preservation_unsupported;
#[cfg(test)]
use preserve::time_preservation_unsupported;
#[cfg(test)]
use preserve::xattr_copy_unsupported;
use preserve::{preserve_metadata, preserve_symlink_times, preserve_xattrs};
use progress::ProgressTracker;
pub use progress::{
    OperationProgress, OperationProgressEvent, OperationProgressPhase, OperationThroughputClass,
    OperationThroughputSnapshot,
};
use recovery::recoverable_operations;
pub use recovery::{OperationRecoveryOutcome, OperationRecoveryPolicy, OperationRecoveryReport};
#[cfg(test)]
use trashmeta::append_trash_metadata_entry;
use trashmeta::{append_trash_metadata, reconcile_empty_trash_metadata, remove_trash_metadata};
pub use trashmeta::{read_trash_metadata, TrashRestoreMetadata};
use verify::verify_copy;
pub use verify::VerificationPolicy;
pub use volume::{OperationVolumeClass, OperationVolumeCopyPolicy};
#[cfg(test)]
use volume::{COPY_BUFFER_BYTES, SLOW_COPY_BUFFER_BYTES};

use gfm_fs::PackagePolicy;
use gfm_types::{FileKind, GfmError, Result};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    Copy { from: PathBuf, to: PathBuf },
    Move { from: PathBuf, to: PathBuf },
    Rename { from: PathBuf, to: PathBuf },
    Delete { path: PathBuf },
    Trash { path: PathBuf },
    EmptyTrash { path: PathBuf },
    Restore { from: PathBuf, to: PathBuf },
}

impl Operation {
    pub fn target_path(&self) -> Option<&Path> {
        match self {
            Self::Copy { to, .. }
            | Self::Move { to, .. }
            | Self::Rename { to, .. }
            | Self::Restore { to, .. } => Some(to),
            Self::Delete { .. } | Self::Trash { .. } | Self::EmptyTrash { .. } => None,
        }
    }

    pub fn access_requirements(&self) -> Vec<OperationAccessRequirement> {
        match self {
            Self::Copy { from, to }
            | Self::Move { from, to }
            | Self::Rename { from, to }
            | Self::Restore { from, to } => vec![
                OperationAccessRequirement {
                    path: from.clone(),
                    role: OperationAccessRole::Source,
                },
                OperationAccessRequirement {
                    path: destination_probe_path(to),
                    role: OperationAccessRole::DestinationParent,
                },
            ],
            Self::Delete { path } | Self::Trash { path } | Self::EmptyTrash { path } => {
                vec![OperationAccessRequirement {
                    path: path.clone(),
                    role: OperationAccessRole::Target,
                }]
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyMethod {
    ApfsClone,
    ByteCopy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyExistingMode {
    Fresh,
    Resume,
    Merge,
}

#[derive(Debug, Clone)]
pub struct OperationContext {
    pub conflict: ConflictPolicy,
    pub journal_path: PathBuf,
    pub trash_metadata_path: Option<PathBuf>,
    pub cancellation: OperationCancellation,
    pub pause: OperationPause,
    pub verification: VerificationPolicy,
    pub access_gate: OperationAccessGate,
    pub volume_copy_policy: OperationVolumeCopyPolicy,
}

impl OperationContext {
    pub fn new(journal_path: impl Into<PathBuf>) -> Self {
        Self {
            conflict: ConflictPolicy::Fail,
            journal_path: journal_path.into(),
            trash_metadata_path: None,
            cancellation: OperationCancellation::default(),
            pause: OperationPause::default(),
            verification: VerificationPolicy::Size,
            access_gate: OperationAccessGate::default(),
            volume_copy_policy: OperationVolumeCopyPolicy::default(),
        }
    }

    pub fn with_conflict(mut self, conflict: ConflictPolicy) -> Self {
        self.conflict = conflict;
        self
    }

    pub fn with_trash_metadata_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.trash_metadata_path = Some(path.into());
        self
    }

    pub fn with_cancellation(mut self, cancellation: OperationCancellation) -> Self {
        self.cancellation = cancellation;
        self
    }

    pub fn with_pause(mut self, pause: OperationPause) -> Self {
        self.pause = pause;
        self
    }

    pub fn with_verification(mut self, verification: VerificationPolicy) -> Self {
        self.verification = verification;
        self
    }

    pub fn with_access_gate(mut self, access_gate: OperationAccessGate) -> Self {
        self.access_gate = access_gate;
        self
    }

    pub fn with_volume_copy_policy(mut self, policy: OperationVolumeCopyPolicy) -> Self {
        self.volume_copy_policy = policy;
        self
    }
}

pub struct Operator {
    context: OperationContext,
}

impl Operator {
    pub fn new(context: OperationContext) -> Self {
        Self { context }
    }

    pub fn execute(&self, operation: Operation) -> Result<JournalEntry> {
        self.execute_with_progress(operation, |_| {})
    }

    pub fn execute_with_progress(
        &self,
        operation: Operation,
        mut on_progress: impl FnMut(OperationProgressEvent),
    ) -> Result<JournalEntry> {
        let operation = resolve_operation_conflicts(operation, self.context.conflict)?;
        let id = now_nanos();
        self.append(JournalEntry::started(id, operation.clone()))?;
        self.execute_started(id, operation, &mut on_progress)
    }

    pub fn execute_batch_with_conflicts(
        &self,
        operations: impl IntoIterator<Item = Operation>,
        plan: OperationConflictPlan,
    ) -> Result<OperationBatchReport> {
        let mut outcomes = Vec::new();
        for operation in operations {
            let conflict = plan.conflict_for(&operation);
            let operator = Operator::new(self.context.clone().with_conflict(conflict));
            match operator.execute(operation.clone()) {
                Ok(entry) => outcomes.push(OperationBatchOutcome {
                    conflict,
                    status: entry.status,
                    operation: entry.operation,
                    message: entry.message,
                }),
                Err(err) => {
                    let status = operation_status_from_error(&err);
                    outcomes.push(OperationBatchOutcome {
                        conflict,
                        status,
                        operation,
                        message: (!matches!(err, GfmError::Cancelled | GfmError::Paused))
                            .then(|| err.to_string()),
                    });
                    if matches!(status, OperationStatus::Cancelled | OperationStatus::Paused) {
                        break;
                    }
                }
            }
        }
        Ok(OperationBatchReport { outcomes })
    }

    pub fn recover_interrupted(&self) -> Result<OperationRecoveryReport> {
        self.recover_with_policy(OperationRecoveryPolicy::default())
    }

    pub fn recover_with_policy(
        &self,
        policy: OperationRecoveryPolicy,
    ) -> Result<OperationRecoveryReport> {
        let recoverable = recoverable_operations(self.journal()?, policy);
        let mut outcomes = Vec::with_capacity(recoverable.len());
        for plan in recoverable {
            let entry = plan.entry;
            let operation = entry.operation;
            if plan.append_started {
                self.append(JournalEntry::started(entry.id, operation.clone()))?;
            }
            match self.execute_started_resuming(entry.id, operation.clone(), &mut |_| {}) {
                Ok(completed) => outcomes.push(OperationRecoveryOutcome {
                    id: completed.id,
                    status: completed.status,
                    operation: completed.operation,
                    message: completed.message,
                }),
                Err(err) => outcomes.push(OperationRecoveryOutcome {
                    id: entry.id,
                    status: operation_status_from_error(&err),
                    operation,
                    message: (!matches!(err, GfmError::Cancelled | GfmError::Paused))
                        .then(|| err.to_string()),
                }),
            }
        }
        Ok(OperationRecoveryReport { outcomes })
    }

    fn execute_started(
        &self,
        id: u128,
        operation: Operation,
        on_progress: &mut impl FnMut(OperationProgressEvent),
    ) -> Result<JournalEntry> {
        self.execute_started_inner(id, operation, false, on_progress)
    }

    fn execute_started_resuming(
        &self,
        id: u128,
        operation: Operation,
        on_progress: &mut impl FnMut(OperationProgressEvent),
    ) -> Result<JournalEntry> {
        self.execute_started_inner(id, operation, true, on_progress)
    }

    fn execute_started_inner(
        &self,
        id: u128,
        operation: Operation,
        resuming: bool,
        on_progress: &mut impl FnMut(OperationProgressEvent),
    ) -> Result<JournalEntry> {
        if let Err(err) = self.context.access_gate.check(&operation) {
            let entry = JournalEntry::from_error(id, operation, &err);
            let _ = self.append(entry);
            return Err(err);
        }
        if should_skip_operation(&operation, self.context.conflict) {
            let entry = JournalEntry::skipped(id, operation);
            self.append(entry.clone())?;
            return Ok(entry);
        }
        let plan = match plan_operation_checked(&operation, &self.context.cancellation) {
            Ok(plan) => plan,
            Err(err) => {
                let entry = JournalEntry::from_error(id, operation, &err);
                let _ = self.append(entry);
                return Err(err);
            }
        };
        let mut progress = ProgressTracker::new(
            plan,
            &self.context.cancellation,
            &self.context.pause,
            on_progress,
        );
        match self.apply(&operation, resuming, &mut progress) {
            Ok(()) => {
                let entry = JournalEntry::completed(id, operation);
                self.append(entry.clone())?;
                Ok(entry)
            }
            Err(err) => {
                let entry = JournalEntry::from_error(id, operation, &err);
                let _ = self.append(entry);
                Err(err)
            }
        }
    }

    pub fn journal(&self) -> Result<Vec<JournalEntry>> {
        read_journal(&self.context.journal_path)
    }

    fn apply(
        &self,
        operation: &Operation,
        resuming: bool,
        progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
    ) -> Result<()> {
        match operation {
            Operation::Copy { from, to } => copy_path(
                from,
                to,
                self.context.conflict,
                self.context.verification,
                &self.context.volume_copy_policy,
                resuming,
                progress,
            ),
            Operation::Move { from, to } | Operation::Rename { from, to } => move_path(
                from,
                to,
                self.context.conflict,
                self.context.verification,
                &self.context.volume_copy_policy,
                resuming,
                progress,
            ),
            Operation::Delete { path } => {
                delete_path(path, self.context.trash_metadata_path.as_deref(), progress)
            }
            Operation::Trash { path } => {
                trash_path(path, self.context.trash_metadata_path.as_deref(), progress)
            }
            Operation::EmptyTrash { path } => {
                empty_trash_path(path, self.context.trash_metadata_path.as_deref(), progress)
            }
            Operation::Restore { from, to } => restore_path(
                from,
                to,
                self.context.conflict,
                &self.context.volume_copy_policy,
                self.context.trash_metadata_path.as_deref(),
                progress,
            ),
        }
    }

    fn append(&self, entry: JournalEntry) -> Result<()> {
        append_journal(&self.context.journal_path, &entry)
    }
}

#[derive(Debug, Default)]
struct CopySession {
    hard_links: BTreeMap<FileIdentity, PathBuf>,
}

impl CopySession {
    fn copied_hard_link_destination(&self, metadata: &fs::Metadata) -> Option<&Path> {
        hard_link_identity(metadata)
            .and_then(|identity| self.hard_links.get(&identity))
            .map(PathBuf::as_path)
    }

    fn remember_hard_link_destination(&mut self, metadata: &fs::Metadata, destination: &Path) {
        if let Some(identity) = hard_link_identity(metadata) {
            self.hard_links
                .entry(identity)
                .or_insert_with(|| destination.to_path_buf());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

#[derive(Debug, Clone, Copy)]
struct CopyExecution<'a> {
    verification: VerificationPolicy,
    volume_copy_policy: &'a OperationVolumeCopyPolicy,
}

#[cfg(unix)]
fn hard_link_identity(metadata: &fs::Metadata) -> Option<FileIdentity> {
    use std::os::unix::fs::MetadataExt;

    if metadata.is_file() && metadata.nlink() > 1 {
        Some(FileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    } else {
        None
    }
}

#[cfg(not(unix))]
fn hard_link_identity(_metadata: &fs::Metadata) -> Option<FileIdentity> {
    None
}

fn copy_path(
    from: &Path,
    to: &Path,
    conflict: ConflictPolicy,
    verification: VerificationPolicy,
    volume_copy_policy: &OperationVolumeCopyPolicy,
    resuming: bool,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    let mut session = CopySession::default();
    let execution = CopyExecution {
        verification,
        volume_copy_policy,
    };
    copy_path_with_session(
        from,
        to,
        conflict,
        execution,
        resuming,
        progress,
        &mut session,
    )
}

fn copy_path_with_session(
    from: &Path,
    to: &Path,
    conflict: ConflictPolicy,
    execution: CopyExecution<'_>,
    resuming: bool,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
    session: &mut CopySession,
) -> Result<()> {
    progress.check_cancelled()?;
    ensure_source_exists(from)?;
    let metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    if resuming && path_exists_or_symlink(to) {
        return copy_path_existing(
            from,
            to,
            &metadata,
            execution,
            CopyExistingMode::Resume,
            progress,
            session,
        );
    }
    if conflict == ConflictPolicy::Merge && metadata.is_dir() && path_exists_or_symlink(to) {
        return copy_path_existing(
            from,
            to,
            &metadata,
            execution,
            CopyExistingMode::Merge,
            progress,
            session,
        );
    }
    if conflict == ConflictPolicy::Replace
        && metadata.is_file()
        && !metadata.file_type().is_symlink()
        && replacement_destination_is_non_directory(to)
    {
        return copy_file_replacing_existing(from, to, execution, progress);
    }
    if conflict == ConflictPolicy::Replace
        && metadata.file_type().is_symlink()
        && replacement_destination_is_non_directory(to)
    {
        return copy_symlink_replacing_existing(from, to, progress);
    }
    if conflict == ConflictPolicy::Replace
        && metadata.is_dir()
        && replacement_destination_is_directory(to)
    {
        return copy_directory_replacing_existing(from, to, execution, progress, session);
    }
    prepare_destination(to, conflict)?;
    if metadata.file_type().is_symlink() {
        copy_symlink(from, to, progress)
    } else if metadata.is_dir() {
        copy_directory(
            from,
            to,
            execution,
            CopyExistingMode::Fresh,
            progress,
            session,
        )
    } else {
        copy_file_with_session(from, to, &metadata, execution, progress, session)?;
        Ok(())
    }
}

fn copy_file_replacing_existing(
    from: &Path,
    to: &Path,
    execution: CopyExecution<'_>,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    let source_metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    let destination_metadata = fs::symlink_metadata(to).map_err(|err| GfmError::io(to, err))?;
    if metadata_same_file(&source_metadata, &destination_metadata) {
        return progress.advance(&source_metadata);
    }
    let stage = allocate_replace_stage_path(to)?;
    let result = (|| {
        copy_file_tracked(
            from,
            &stage,
            execution.verification,
            execution.volume_copy_policy,
            progress,
        )?;
        rename_replacing_file(&stage, to)
    })();
    if result.is_err() && path_exists_or_symlink(&stage) {
        let _ = delete_path_untracked(&stage);
    }
    result
}

fn copy_directory_replacing_existing(
    from: &Path,
    to: &Path,
    execution: CopyExecution<'_>,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
    session: &mut CopySession,
) -> Result<()> {
    progress.check_cancelled()?;
    let source_metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    let destination_metadata = fs::symlink_metadata(to).map_err(|err| GfmError::io(to, err))?;
    if metadata_same_file(&source_metadata, &destination_metadata) {
        return progress.complete();
    }
    let stage = allocate_replace_stage_path(to)?;
    let backup = allocate_replace_backup_path(to)?;
    let result = (|| {
        copy_directory(
            from,
            &stage,
            execution,
            CopyExistingMode::Fresh,
            progress,
            session,
        )?;
        commit_staged_directory_replace(&stage, to, &backup)
    })();
    if result.is_err() && path_exists_or_symlink(&stage) {
        let _ = delete_path_untracked(&stage);
    }
    result
}

fn copy_symlink_replacing_existing(
    from: &Path,
    to: &Path,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    let source_metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    let destination_metadata = fs::symlink_metadata(to).map_err(|err| GfmError::io(to, err))?;
    if metadata_same_file(&source_metadata, &destination_metadata) {
        return progress.advance(&source_metadata);
    }
    let stage = allocate_replace_stage_path(to)?;
    let result = (|| {
        copy_symlink(from, &stage, progress)?;
        rename_replacing_file(&stage, to)
    })();
    if result.is_err() && path_exists_or_symlink(&stage) {
        let _ = delete_path_untracked(&stage);
    }
    result
}

#[cfg(unix)]
fn metadata_same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn metadata_same_file(_left: &fs::Metadata, _right: &fs::Metadata) -> bool {
    false
}

fn replacement_destination_is_non_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| !metadata.is_dir())
        .unwrap_or(false)
}

fn replacement_destination_is_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
}

fn allocate_replace_stage_path(to: &Path) -> Result<PathBuf> {
    allocate_hidden_sibling_path(to, "gfm-replace")
}

fn allocate_replace_backup_path(to: &Path) -> Result<PathBuf> {
    allocate_hidden_sibling_path(to, "gfm-replaced")
}

fn allocate_hidden_sibling_path(to: &Path, label: &str) -> Result<PathBuf> {
    let parent = to.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    let file_name = to
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("copy");
    let nonce = now_nanos();
    for attempt in 0..100_u32 {
        let candidate = parent.join(format!(".{}.{}-{}-{}", file_name, label, nonce, attempt));
        if !path_exists_or_symlink(&candidate) {
            return Ok(candidate);
        }
    }
    Err(GfmError::Conflict {
        path: to.to_path_buf(),
        message: format!("could not allocate a safe {label} sibling path"),
    })
}

fn rename_replacing_file(from: &Path, to: &Path) -> Result<()> {
    fs::rename(from, to).map_err(|err| GfmError::io(to, err))
}

fn commit_staged_directory_replace(stage: &Path, to: &Path, backup: &Path) -> Result<()> {
    fs::rename(to, backup).map_err(|err| GfmError::io(to, err))?;
    match fs::rename(stage, to) {
        Ok(()) => {
            if let Err(cleanup_err) = delete_path_untracked(backup) {
                return Err(GfmError::Format(format!(
                    "replaced {} but failed to remove previous destination backup {}: {}",
                    to.display(),
                    backup.display(),
                    cleanup_err
                )));
            }
            Ok(())
        }
        Err(replace_err) => {
            let restore_result = fs::rename(backup, to);
            if let Err(restore_err) = restore_result {
                return Err(GfmError::Format(format!(
                    "failed to install staged replacement {} -> {}: {}; also failed to restore previous destination {} -> {}: {}",
                    stage.display(),
                    to.display(),
                    replace_err,
                    backup.display(),
                    to.display(),
                    restore_err
                )));
            }
            Err(GfmError::io(to, replace_err))
        }
    }
}

fn move_path(
    from: &Path,
    to: &Path,
    conflict: ConflictPolicy,
    verification: VerificationPolicy,
    volume_copy_policy: &OperationVolumeCopyPolicy,
    resuming: bool,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    ensure_source_exists(from)?;
    if resuming && path_exists_or_symlink(to) {
        let metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
        let execution = CopyExecution {
            verification,
            volume_copy_policy,
        };
        copy_path_existing(
            from,
            to,
            &metadata,
            execution,
            CopyExistingMode::Resume,
            progress,
            &mut CopySession::default(),
        )?;
        delete_path_untracked(from)?;
        return Ok(());
    }
    if conflict == ConflictPolicy::Merge && path_exists_or_symlink(to) {
        let metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
        let execution = CopyExecution {
            verification,
            volume_copy_policy,
        };
        copy_path_existing(
            from,
            to,
            &metadata,
            execution,
            CopyExistingMode::Merge,
            progress,
            &mut CopySession::default(),
        )?;
        delete_path_untracked(from)?;
        return Ok(());
    }
    if conflict == ConflictPolicy::Replace
        && source_is_regular_file(from)
        && replacement_destination_is_non_directory(to)
    {
        return move_file_replacing_existing(from, to, verification, volume_copy_policy, progress);
    }
    if conflict == ConflictPolicy::Replace
        && source_is_symlink(from)
        && replacement_destination_is_non_directory(to)
    {
        return move_symlink_replacing_existing(from, to, progress);
    }
    if conflict == ConflictPolicy::Replace
        && source_is_directory(from)
        && replacement_destination_is_directory(to)
    {
        return move_directory_replacing_existing(
            from,
            to,
            verification,
            volume_copy_policy,
            progress,
        );
    }
    prepare_destination(to, conflict)?;
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    match fs::rename(from, to) {
        Ok(()) => progress.complete(),
        Err(rename_err) => {
            copy_path(
                from,
                to,
                ConflictPolicy::Replace,
                verification,
                volume_copy_policy,
                false,
                progress,
            )?;
            delete_path_untracked(from).map_err(|delete_err| {
                GfmError::Format(format!(
                    "moved copy to {} but failed to remove source {}: {}; original rename error: {}",
                    to.display(),
                    from.display(),
                    delete_err,
                    rename_err
                ))
            })
        }
    }
}

fn move_file_replacing_existing(
    from: &Path,
    to: &Path,
    verification: VerificationPolicy,
    volume_copy_policy: &OperationVolumeCopyPolicy,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    let source_metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    let destination_metadata = fs::symlink_metadata(to).map_err(|err| GfmError::io(to, err))?;
    if metadata_same_file(&source_metadata, &destination_metadata) {
        if !same_canonical_path(from, to) {
            delete_path_untracked(from)?;
        }
        return progress.complete();
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    match fs::rename(from, to) {
        Ok(()) => progress.complete(),
        Err(rename_err) => {
            copy_path(
                from,
                to,
                ConflictPolicy::Replace,
                verification,
                volume_copy_policy,
                false,
                progress,
            )?;
            delete_path_untracked(from).map_err(|delete_err| {
                GfmError::Format(format!(
                    "moved copy to {} but failed to remove source {}: {}; original rename error: {}",
                    to.display(),
                    from.display(),
                    delete_err,
                    rename_err
                ))
            })
        }
    }
}

fn move_directory_replacing_existing(
    from: &Path,
    to: &Path,
    verification: VerificationPolicy,
    volume_copy_policy: &OperationVolumeCopyPolicy,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    let source_metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    let destination_metadata = fs::symlink_metadata(to).map_err(|err| GfmError::io(to, err))?;
    if metadata_same_file(&source_metadata, &destination_metadata) {
        return progress.complete();
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    match fs::rename(from, to) {
        Ok(()) => progress.complete(),
        Err(rename_err) => {
            let execution = CopyExecution {
                verification,
                volume_copy_policy,
            };
            let mut session = CopySession::default();
            copy_directory_replacing_existing(from, to, execution, progress, &mut session)?;
            delete_path_untracked(from).map_err(|delete_err| {
                GfmError::Format(format!(
                    "moved directory to {} but failed to remove source {}: {}; original rename error: {}",
                    to.display(),
                    from.display(),
                    delete_err,
                    rename_err
                ))
            })
        }
    }
}

fn move_symlink_replacing_existing(
    from: &Path,
    to: &Path,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    let source_metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    let destination_metadata = fs::symlink_metadata(to).map_err(|err| GfmError::io(to, err))?;
    if metadata_same_file(&source_metadata, &destination_metadata) {
        if from != to {
            delete_path_untracked(from)?;
        }
        return progress.complete();
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    match fs::rename(from, to) {
        Ok(()) => progress.complete(),
        Err(rename_err) => {
            copy_symlink_replacing_existing(from, to, progress)?;
            delete_path_untracked(from).map_err(|delete_err| {
                GfmError::Format(format!(
                    "moved symlink to {} but failed to remove source {}: {}; original rename error: {}",
                    to.display(),
                    from.display(),
                    delete_err,
                    rename_err
                ))
            })
        }
    }
}

fn source_is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
}

fn source_is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
}

fn source_is_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
}

fn same_canonical_path(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn delete_path(
    path: &Path,
    metadata_path: Option<&Path>,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    let metadata = fs::symlink_metadata(path).map_err(|err| GfmError::io(path, err))?;
    if metadata.is_dir() {
        fs::remove_dir_all(path).map_err(|err| GfmError::io(path, err))?;
        remove_deleted_trash_metadata(metadata_path, path)?;
        progress.complete()
    } else {
        fs::remove_file(path).map_err(|err| GfmError::io(path, err))?;
        remove_deleted_trash_metadata(metadata_path, path)?;
        progress.advance(&metadata)
    }
}

fn delete_path_untracked(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|err| GfmError::io(path, err))?;
    if metadata.is_dir() {
        fs::remove_dir_all(path).map_err(|err| GfmError::io(path, err))
    } else {
        fs::remove_file(path).map_err(|err| GfmError::io(path, err))
    }
}

fn trash_path(
    path: &Path,
    metadata_path: Option<&Path>,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    ensure_source_exists(path)?;
    if let Some(metadata_path) = metadata_path {
        append_trash_metadata(metadata_path, path)?;
    }
    trash::delete(path).map_err(|err| GfmError::io(path, err))?;
    progress.complete()
}

fn empty_trash_path(
    path: &Path,
    metadata_path: Option<&Path>,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    let metadata = fs::symlink_metadata(path).map_err(|err| GfmError::io(path, err))?;
    if !metadata.is_dir() {
        return Err(GfmError::Format(format!(
            "empty trash requires a directory: {}",
            path.display()
        )));
    }
    let mut entries = fs::read_dir(path)
        .map_err(|err| GfmError::io(path, err))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|err| GfmError::io(path, err))
        })
        .collect::<Result<Vec<_>>>()?;
    entries.sort();
    for entry in entries {
        delete_trash_child(&entry, metadata_path, progress)?;
    }
    reconcile_empty_trash_metadata(metadata_path, path)?;
    progress.complete()
}

fn delete_trash_child(
    path: &Path,
    metadata_path: Option<&Path>,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    let metadata = fs::symlink_metadata(path).map_err(|err| GfmError::io(path, err))?;
    delete_path_untracked(path)?;
    remove_deleted_trash_metadata(metadata_path, path)?;
    if metadata.is_dir() {
        progress.finish_current_item()
    } else {
        progress.advance(&metadata)
    }
}

fn restore_path(
    from: &Path,
    to: &Path,
    conflict: ConflictPolicy,
    volume_copy_policy: &OperationVolumeCopyPolicy,
    metadata_path: Option<&Path>,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    move_path(
        from,
        to,
        conflict,
        VerificationPolicy::Bytes,
        volume_copy_policy,
        false,
        progress,
    )?;
    if let Some(metadata_path) = metadata_path {
        remove_trash_metadata(metadata_path, from)?;
    }
    Ok(())
}

fn remove_deleted_trash_metadata(metadata_path: Option<&Path>, deleted_path: &Path) -> Result<()> {
    if let Some(metadata_path) = metadata_path {
        remove_trash_metadata(metadata_path, deleted_path)?;
    }
    Ok(())
}

fn copy_directory(
    from: &Path,
    to: &Path,
    execution: CopyExecution<'_>,
    mode: CopyExistingMode,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
    session: &mut CopySession,
) -> Result<()> {
    progress.check_cancelled()?;
    let metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    let rollback_incomplete_fresh_destination =
        mode == CopyExistingMode::Fresh && metadata.is_dir();
    let mut created_destination = false;
    let result = (|| {
        if mode != CopyExistingMode::Fresh && path_exists_or_symlink(to) {
            let destination_metadata =
                fs::symlink_metadata(to).map_err(|err| GfmError::io(to, err))?;
            if !destination_metadata.is_dir() {
                return Err(GfmError::Conflict {
                    path: to.to_path_buf(),
                    message: format!(
                        "{} destination exists but is not a directory",
                        copy_mode_label(mode)
                    ),
                });
            }
        } else {
            create_new_directory(to)?;
            created_destination = true;
        }
        progress.advance(&metadata)?;

        for entry in fs::read_dir(from).map_err(|err| GfmError::io(from, err))? {
            progress.check_cancelled()?;
            let entry = entry.map_err(|err| GfmError::io(from, err))?;
            let source = entry.path();
            let destination = to.join(entry.file_name());
            let child_metadata =
                fs::symlink_metadata(&source).map_err(|err| GfmError::io(&source, err))?;
            if child_metadata.file_type().is_symlink() {
                if mode == CopyExistingMode::Resume && path_exists_or_symlink(&destination) {
                    copy_path_existing(
                        &source,
                        &destination,
                        &child_metadata,
                        execution,
                        CopyExistingMode::Resume,
                        progress,
                        session,
                    )?;
                } else if mode == CopyExistingMode::Merge && path_exists_or_symlink(&destination) {
                    copy_path_existing(
                        &source,
                        &destination,
                        &child_metadata,
                        execution,
                        CopyExistingMode::Merge,
                        progress,
                        session,
                    )?;
                } else {
                    copy_symlink(&source, &destination, progress)?;
                }
            } else if child_metadata.is_dir() {
                copy_directory(&source, &destination, execution, mode, progress, session)?;
            } else if mode == CopyExistingMode::Resume && path_exists_or_symlink(&destination) {
                copy_path_existing(
                    &source,
                    &destination,
                    &child_metadata,
                    execution,
                    CopyExistingMode::Resume,
                    progress,
                    session,
                )?;
            } else if mode == CopyExistingMode::Merge && path_exists_or_symlink(&destination) {
                copy_path_existing(
                    &source,
                    &destination,
                    &child_metadata,
                    execution,
                    CopyExistingMode::Merge,
                    progress,
                    session,
                )?;
            } else {
                copy_file_with_session(
                    &source,
                    &destination,
                    &child_metadata,
                    execution,
                    progress,
                    session,
                )?;
            }
        }
        preserve_metadata(from, to, &metadata)?;
        Ok(())
    })();
    if rollback_incomplete_fresh_destination
        && created_destination
        && result
            .as_ref()
            .is_err_and(|err| !matches!(err, GfmError::Paused))
    {
        let _ = delete_path_untracked(to);
    }
    result
}

fn copy_path_existing(
    from: &Path,
    to: &Path,
    metadata: &fs::Metadata,
    execution: CopyExecution<'_>,
    mode: CopyExistingMode,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
    session: &mut CopySession,
) -> Result<()> {
    progress.check_cancelled()?;
    if metadata.file_type().is_symlink() {
        verify_existing_symlink_copy(from, to, mode)?;
        progress.advance(metadata)
    } else if metadata.is_dir() {
        if mode == CopyExistingMode::Merge
            && (is_finder_package_dir(from, metadata) || is_existing_finder_package_dir(to))
        {
            return Err(GfmError::Conflict {
                path: to.to_path_buf(),
                message: "merge destination package already exists".to_string(),
            });
        }
        copy_directory(from, to, execution, mode, progress, session)
    } else if mode == CopyExistingMode::Merge {
        Err(GfmError::Conflict {
            path: to.to_path_buf(),
            message: "merge destination file already exists".to_string(),
        })
    } else {
        verify_copy(from, to, execution.verification)?;
        preserve_metadata(from, to, metadata)?;
        progress.advance(metadata)
    }
}

fn is_existing_finder_package_dir(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| is_finder_package_dir(path, &metadata))
        .unwrap_or(false)
}

fn is_finder_package_dir(path: &Path, metadata: &fs::Metadata) -> bool {
    metadata.is_dir()
        && PackagePolicy::default()
            .classify(path, FileKind::Directory)
            .is_some()
}

fn verify_existing_symlink_copy(from: &Path, to: &Path, mode: CopyExistingMode) -> Result<()> {
    if mode == CopyExistingMode::Merge {
        return Err(GfmError::Conflict {
            path: to.to_path_buf(),
            message: "merge destination symlink already exists".to_string(),
        });
    }
    let source_target = fs::read_link(from).map_err(|err| GfmError::io(from, err))?;
    let destination_target = fs::read_link(to).map_err(|err| GfmError::io(to, err))?;
    if source_target == destination_target {
        Ok(())
    } else {
        Err(GfmError::Conflict {
            path: to.to_path_buf(),
            message: format!(
                "resume symlink target mismatch: {} != {}",
                source_target.display(),
                destination_target.display()
            ),
        })
    }
}

fn create_new_directory(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Err(GfmError::Conflict {
            path: path.to_path_buf(),
            message: "destination directory already exists".to_string(),
        }),
        Err(err) => Err(GfmError::io(path, err)),
    }
}

fn copy_mode_label(mode: CopyExistingMode) -> &'static str {
    match mode {
        CopyExistingMode::Fresh => "fresh",
        CopyExistingMode::Resume => "resume",
        CopyExistingMode::Merge => "merge",
    }
}

fn copy_symlink(
    from: &Path,
    to: &Path,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    let target = fs::read_link(from).map_err(|err| GfmError::io(from, err))?;
    create_symlink(&target, to)?;
    let metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    preserve_xattrs(from, to)?;
    preserve_symlink_times(to, &metadata)?;
    progress.advance(&metadata)
}

#[cfg(test)]
fn copy_file(from: &Path, to: &Path, verification: VerificationPolicy) -> Result<CopyMethod> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    let metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    match clone_file(from, to) {
        Ok(()) => {
            preserve_metadata(from, to, &metadata)?;
            verify_copy(from, to, verification)?;
            Ok(CopyMethod::ApfsClone)
        }
        Err(err) if clone_fallback_allowed(&err) => {
            remove_failed_clone_destination(to)?;
            copy_file_bytes(from, to)?;
            preserve_metadata(from, to, &metadata)?;
            verify_copy(from, to, verification)?;
            Ok(CopyMethod::ByteCopy)
        }
        Err(err) => Err(GfmError::io(from, err)),
    }
}

fn copy_file_tracked(
    from: &Path,
    to: &Path,
    verification: VerificationPolicy,
    volume_copy_policy: &OperationVolumeCopyPolicy,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<CopyMethod> {
    progress.check_cancelled()?;
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    let metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    match clone_file(from, to) {
        Ok(()) => {
            preserve_metadata(from, to, &metadata)?;
            verify_copy(from, to, verification)?;
            progress.advance(&metadata)?;
            Ok(CopyMethod::ApfsClone)
        }
        Err(err) if clone_fallback_allowed(&err) => {
            remove_failed_clone_destination(to)?;
            copy_file_bytes_tracked(from, to, volume_copy_policy, progress)?;
            preserve_metadata(from, to, &metadata)?;
            verify_copy(from, to, verification)?;
            progress.finish_current_item()?;
            Ok(CopyMethod::ByteCopy)
        }
        Err(err) => Err(GfmError::io(from, err)),
    }
}

fn copy_file_with_session(
    from: &Path,
    to: &Path,
    metadata: &fs::Metadata,
    execution: CopyExecution<'_>,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
    session: &mut CopySession,
) -> Result<()> {
    if let Some(existing) = session
        .copied_hard_link_destination(metadata)
        .map(Path::to_path_buf)
    {
        link_existing_hard_link(&existing, to, metadata, progress)?;
        return Ok(());
    }

    copy_file_tracked(
        from,
        to,
        execution.verification,
        execution.volume_copy_policy,
        progress,
    )?;
    session.remember_hard_link_destination(metadata, to);
    Ok(())
}

fn link_existing_hard_link(
    existing: &Path,
    to: &Path,
    metadata: &fs::Metadata,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    fs::hard_link(existing, to).map_err(|err| GfmError::io(to, err))?;
    progress.advance(metadata)
}

#[cfg(test)]
fn copy_file_bytes(from: &Path, to: &Path) -> Result<u64> {
    let mut source = File::open(from).map_err(|err| GfmError::io(from, err))?;
    let source_metadata = source.metadata().map_err(|err| GfmError::io(from, err))?;
    let preserve_sparse_holes = metadata_has_sparse_holes(&source_metadata);
    let mut destination = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)
        .map_err(|err| GfmError::io(to, err))?;
    let mut buffer = vec![0; COPY_BUFFER_BYTES];
    let mut written = 0_u64;

    let result = loop {
        let read = match source.read(&mut buffer) {
            Ok(read) => read,
            Err(err) => break Err(GfmError::io(from, err)),
        };
        if read == 0 {
            break Ok(written);
        }
        if let Err(err) = write_copy_chunk(&mut destination, &buffer[..read], preserve_sparse_holes)
        {
            break Err(GfmError::io(to, err));
        }
        written += read as u64;
    };

    let result = result.and_then(|written| {
        destination
            .set_len(written)
            .map_err(|err| GfmError::io(to, err))?;
        Ok(written)
    });

    if result.is_err() {
        let _ = fs::remove_file(to);
    }
    result
}

fn copy_file_bytes_tracked(
    from: &Path,
    to: &Path,
    volume_copy_policy: &OperationVolumeCopyPolicy,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<u64> {
    let mut source = File::open(from).map_err(|err| GfmError::io(from, err))?;
    let source_metadata = source.metadata().map_err(|err| GfmError::io(from, err))?;
    let preserve_sparse_holes = metadata_has_sparse_holes(&source_metadata);
    let mut destination = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)
        .map_err(|err| GfmError::io(to, err))?;
    let mut buffer = vec![0; volume_copy_policy.copy_buffer_bytes_for_paths(from, to)];
    let mut written = 0_u64;

    let result = loop {
        progress.check_cancelled()?;
        let read = match source.read(&mut buffer) {
            Ok(read) => read,
            Err(err) => break Err(GfmError::io(from, err)),
        };
        if read == 0 {
            break Ok(written);
        }
        if let Err(err) = write_copy_chunk(&mut destination, &buffer[..read], preserve_sparse_holes)
        {
            break Err(GfmError::io(to, err));
        }
        written += read as u64;
        if let Err(err) = progress.advance_bytes(read as u64) {
            break Err(err);
        }
    };

    let result = result.and_then(|written| {
        destination
            .set_len(written)
            .map_err(|err| GfmError::io(to, err))?;
        Ok(written)
    });

    if result.is_err() {
        let _ = fs::remove_file(to);
    }
    result
}

fn write_copy_chunk(
    destination: &mut File,
    chunk: &[u8],
    preserve_sparse_holes: bool,
) -> io::Result<()> {
    if preserve_sparse_holes {
        write_sparse_chunk(destination, chunk)
    } else {
        destination.write_all(chunk)
    }
}

fn write_sparse_chunk(destination: &mut File, chunk: &[u8]) -> io::Result<()> {
    let mut cursor = 0;
    while cursor < chunk.len() {
        let run_start = cursor;
        if chunk[cursor] == 0 {
            while cursor < chunk.len() && chunk[cursor] == 0 {
                cursor += 1;
            }
            destination.seek(SeekFrom::Current((cursor - run_start) as i64))?;
        } else {
            while cursor < chunk.len() && chunk[cursor] != 0 {
                cursor += 1;
            }
            destination.write_all(&chunk[run_start..cursor])?;
        }
    }
    Ok(())
}

fn metadata_has_sparse_holes(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        metadata.len() > 0 && metadata.blocks().saturating_mul(512) < metadata.len()
    }

    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link).map_err(|err| GfmError::io(link, err))
}

#[cfg(windows)]
fn create_symlink(target: &Path, link: &Path) -> Result<()> {
    if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, link).map_err(|err| GfmError::io(link, err))
    } else {
        std::os::windows::fs::symlink_file(target, link).map_err(|err| GfmError::io(link, err))
    }
}

#[cfg(target_os = "macos")]
fn clone_file(from: &Path, to: &Path) -> io::Result<()> {
    let source = File::open(from)?;
    rustix::fs::fclonefileat(
        &source,
        rustix::fs::CWD,
        to,
        rustix::fs::CloneFlags::empty(),
    )
    .map_err(io::Error::from)
}

#[cfg(not(target_os = "macos"))]
fn clone_file(_from: &Path, _to: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "native clonefile is only available on macOS",
    ))
}

fn clone_fallback_allowed(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(libc::ENOTSUP) | Some(libc::EXDEV) | Some(libc::EINVAL)
    ) || matches!(
        err.kind(),
        io::ErrorKind::Unsupported | io::ErrorKind::InvalidInput
    )
}

fn remove_failed_clone_destination(to: &Path) -> Result<()> {
    match fs::symlink_metadata(to) {
        Ok(metadata) if metadata.is_dir() => {
            fs::remove_dir_all(to).map_err(|err| GfmError::io(to, err))
        }
        Ok(_) => fs::remove_file(to).map_err(|err| GfmError::io(to, err)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(GfmError::io(to, err)),
    }
}

fn ensure_source_exists(path: &Path) -> Result<()> {
    if path.exists() || fs::symlink_metadata(path).is_ok() {
        Ok(())
    } else {
        Err(GfmError::Io {
            path: path.to_path_buf(),
            message: "source does not exist".to_string(),
        })
    }
}

fn resolve_operation_conflicts(
    operation: Operation,
    conflict: ConflictPolicy,
) -> Result<Operation> {
    if conflict != ConflictPolicy::KeepBoth {
        return Ok(operation);
    }
    match operation {
        Operation::Copy { from, to } => Ok(Operation::Copy {
            from,
            to: keep_both_path(&to)?,
        }),
        Operation::Move { from, to } => Ok(Operation::Move {
            from,
            to: keep_both_path(&to)?,
        }),
        Operation::Rename { from, to } => Ok(Operation::Rename {
            from,
            to: keep_both_path(&to)?,
        }),
        Operation::Delete { path } => Ok(Operation::Delete { path }),
        Operation::Trash { path } => Ok(Operation::Trash { path }),
        Operation::EmptyTrash { path } => Ok(Operation::EmptyTrash { path }),
        Operation::Restore { from, to } => Ok(Operation::Restore {
            from,
            to: keep_both_path(&to)?,
        }),
    }
}

fn should_skip_operation(operation: &Operation, conflict: ConflictPolicy) -> bool {
    conflict == ConflictPolicy::Skip && operation.target_path().is_some_and(path_exists_or_symlink)
}

fn keep_both_path(path: &Path) -> Result<PathBuf> {
    if !path_exists_or_symlink(path) {
        return Ok(path.to_path_buf());
    }
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .or_else(|| path.file_name().and_then(|name| name.to_str()))
        .ok_or_else(|| {
            GfmError::Format(format!(
                "could not derive keep-both destination name for {}",
                path.display()
            ))
        })?;
    let extension = path.extension().and_then(|extension| extension.to_str());
    for index in 1..=10_000 {
        let suffix = if index == 1 {
            " copy".to_string()
        } else {
            format!(" copy {index}")
        };
        let candidate_name = match extension {
            Some(extension) if path.file_stem().is_some() => {
                format!("{stem}{suffix}.{extension}")
            }
            _ => format!("{stem}{suffix}"),
        };
        let candidate = parent.join(candidate_name);
        if !path_exists_or_symlink(&candidate) {
            return Ok(candidate);
        }
    }
    Err(GfmError::Conflict {
        path: path.to_path_buf(),
        message: "could not allocate a keep-both destination name".to_string(),
    })
}

fn path_exists_or_symlink(path: &Path) -> bool {
    path.exists() || fs::symlink_metadata(path).is_ok()
}

fn prepare_destination(path: &Path, conflict: ConflictPolicy) -> Result<()> {
    if !path_exists_or_symlink(path) {
        return Ok(());
    }

    match conflict {
        ConflictPolicy::Fail => Err(GfmError::Conflict {
            path: path.to_path_buf(),
            message: "destination already exists".to_string(),
        }),
        ConflictPolicy::Replace => delete_path_untracked(path),
        ConflictPolicy::KeepBoth => Err(GfmError::Conflict {
            path: path.to_path_buf(),
            message: "keep-both destination still exists".to_string(),
        }),
        ConflictPolicy::Merge => Err(GfmError::Conflict {
            path: path.to_path_buf(),
            message: "merge requires source and destination directories".to_string(),
        }),
        ConflictPolicy::Skip => Err(GfmError::Conflict {
            path: path.to_path_buf(),
            message: "skip policy must be handled before mutation".to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_directories_and_records_journal() {
        let root = unique_temp_dir("gfm-ops-copy");
        let journal = root.join("journal.log");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("nested").join("file.txt"), "hello").unwrap();

        let operator = Operator::new(OperationContext::new(&journal));
        let entry = operator
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        assert_eq!(entry.status, OperationStatus::Completed);
        assert_eq!(
            fs::read_to_string(destination.join("nested").join("file.txt")).unwrap(),
            "hello"
        );
        let journal_entries = operator.journal().unwrap();
        assert_eq!(journal_entries.len(), 2);
        assert_eq!(journal_entries[0].status, OperationStatus::Started);
        assert_eq!(journal_entries[1].status, OperationStatus::Completed);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn plans_recursive_copy_totals_before_execution() {
        let root = unique_temp_dir("gfm-ops-plan-copy");
        let source = root.join("source");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("alpha.txt"), "alpha").unwrap();
        fs::write(source.join("nested").join("beta.txt"), "beta").unwrap();

        let progress = plan_operation(&Operation::Copy {
            from: source.clone(),
            to: root.join("destination"),
        })
        .unwrap();

        assert_eq!(progress.total_items, 4);
        assert_eq!(progress.total_bytes, 9);
        assert_eq!(progress.completed_items, 0);
        assert_eq!(progress.completed_bytes, 0);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_emits_planned_and_advanced_progress() {
        let root = unique_temp_dir("gfm-ops-progress-copy");
        let journal = root.join("journal.log");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("first.txt"), "first").unwrap();
        fs::write(source.join("nested").join("second.txt"), "second").unwrap();
        let mut events = Vec::new();

        Operator::new(OperationContext::new(&journal))
            .execute_with_progress(
                Operation::Copy {
                    from: source.clone(),
                    to: destination,
                },
                |event| events.push(event),
            )
            .unwrap();

        assert_eq!(
            events.first().unwrap().phase,
            OperationProgressPhase::Planned
        );
        assert_eq!(events.first().unwrap().progress.total_items, 4);
        assert_eq!(events.first().unwrap().progress.total_bytes, 11);
        let last = events.last().unwrap();
        assert_eq!(last.phase, OperationProgressPhase::Advanced);
        assert_eq!(last.progress.completed_items, 4);
        assert_eq!(last.progress.completed_bytes, 11);
        assert_eq!(last.progress.completed_items, last.progress.total_items);
        assert_eq!(last.progress.completed_bytes, last.progress.total_bytes);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delete_completes_recursive_progress_after_success() {
        let root = unique_temp_dir("gfm-ops-progress-delete");
        let journal = root.join("journal.log");
        let target = root.join("target");
        fs::create_dir_all(target.join("nested")).unwrap();
        fs::write(target.join("nested").join("payload.txt"), "payload").unwrap();
        let mut events = Vec::new();

        Operator::new(OperationContext::new(&journal))
            .execute_with_progress(Operation::Delete { path: target }, |event| {
                events.push(event);
            })
            .unwrap();

        assert_eq!(
            events.first().unwrap().phase,
            OperationProgressPhase::Planned
        );
        assert_eq!(events.first().unwrap().progress.total_items, 3);
        assert_eq!(events.first().unwrap().progress.total_bytes, 7);
        let last = events.last().unwrap();
        assert_eq!(last.phase, OperationProgressPhase::Advanced);
        assert_eq!(last.progress.completed_items, 3);
        assert_eq!(last.progress.completed_bytes, 7);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelled_before_preflight_journals_cancelled_without_progress() {
        let root = unique_temp_dir("gfm-ops-cancel-preflight");
        let journal = root.join("journal.log");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("payload.txt"), "payload").unwrap();
        let cancellation = OperationCancellation::default();
        cancellation.cancel();
        let mut events = Vec::new();

        let err = Operator::new(OperationContext::new(&journal).with_cancellation(cancellation))
            .execute_with_progress(
                Operation::Copy {
                    from: source,
                    to: destination,
                },
                |event| events.push(event),
            )
            .unwrap_err();

        assert!(matches!(err, GfmError::Cancelled));
        assert!(events.is_empty());
        let journal_entries = read_journal(&journal).unwrap();
        assert_eq!(journal_entries.len(), 2);
        assert_eq!(journal_entries[0].status, OperationStatus::Started);
        assert_eq!(journal_entries[1].status, OperationStatus::Cancelled);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recursive_copy_stops_after_cancellation_checkpoint() {
        let root = unique_temp_dir("gfm-ops-cancel-copy");
        let journal = root.join("journal.log");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("first.txt"), "first").unwrap();
        fs::write(source.join("nested").join("second.txt"), "second").unwrap();
        let cancellation = OperationCancellation::default();
        let cancellation_callback = cancellation.clone();
        let mut events = Vec::new();

        let err = Operator::new(OperationContext::new(&journal).with_cancellation(cancellation))
            .execute_with_progress(
                Operation::Copy {
                    from: source,
                    to: destination.clone(),
                },
                |event| {
                    events.push(event);
                    if event.phase == OperationProgressPhase::Advanced
                        && event.progress.completed_items == 1
                    {
                        cancellation_callback.cancel();
                    }
                },
            )
            .unwrap_err();

        assert!(matches!(err, GfmError::Cancelled));
        assert!(!path_exists_or_symlink(&destination));
        assert_eq!(
            events
                .iter()
                .filter(|event| event.phase == OperationProgressPhase::Advanced)
                .count(),
            1
        );
        let journal_entries = read_journal(&journal).unwrap();
        assert_eq!(
            journal_entries.last().unwrap().status,
            OperationStatus::Cancelled
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn paused_recursive_copy_journals_recoverable_pause() {
        let root = unique_temp_dir("gfm-ops-pause-copy");
        let journal = root.join("journal.log");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("nested").join("second.txt"), "second").unwrap();
        fs::write(source.join("first.txt"), "first").unwrap();
        let pause = OperationPause::default();
        let pause_callback = pause.clone();
        let mut events = Vec::new();

        let err = Operator::new(OperationContext::new(&journal).with_pause(pause))
            .execute_with_progress(
                Operation::Copy {
                    from: source.clone(),
                    to: destination.clone(),
                },
                |event| {
                    events.push(event);
                    if event.phase == OperationProgressPhase::Advanced
                        && event.progress.completed_items == 1
                    {
                        pause_callback.pause();
                    }
                },
            )
            .unwrap_err();

        assert!(matches!(err, GfmError::Paused));
        assert!(destination.is_dir());
        assert!(!destination.join("first.txt").exists());
        let entries = read_journal(&journal).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].status, OperationStatus::Started);
        assert_eq!(entries[1].status, OperationStatus::Paused);
        assert!(events
            .iter()
            .any(|event| event.phase == OperationProgressPhase::Advanced
                && event.progress.completed_items == 1));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_resumes_paused_directory_copy_into_existing_destination() {
        let root = unique_temp_dir("gfm-ops-resume-paused-copy");
        let journal = root.join("journal.log");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("first.txt"), "first").unwrap();
        fs::write(source.join("nested").join("second.txt"), "second").unwrap();
        fs::create_dir_all(&destination).unwrap();
        let operation = Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        };
        append_journal(&journal, &JournalEntry::started(46, operation.clone())).unwrap();
        append_journal(&journal, &JournalEntry::paused(46, operation)).unwrap();

        let report = Operator::new(OperationContext::new(&journal))
            .recover_interrupted()
            .unwrap();

        assert_eq!(report.outcomes.len(), 1);
        assert_eq!(report.outcomes[0].id, 46);
        assert_eq!(report.outcomes[0].status, OperationStatus::Completed);
        assert_eq!(
            fs::read_to_string(destination.join("first.txt")).unwrap(),
            "first"
        );
        assert_eq!(
            fs::read_to_string(destination.join("nested").join("second.txt")).unwrap(),
            "second"
        );
        let entries = read_journal(&journal).unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].status, OperationStatus::Started);
        assert_eq!(entries[1].status, OperationStatus::Paused);
        assert_eq!(entries[2].status, OperationStatus::Started);
        assert_eq!(entries[3].status, OperationStatus::Completed);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_file_reports_method_and_preserves_contents() {
        let root = unique_temp_dir("gfm-ops-copy-method");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "clone-aware copy").unwrap();

        let method = copy_file(&source, &destination, VerificationPolicy::Bytes).unwrap();

        assert!(matches!(
            method,
            CopyMethod::ApfsClone | CopyMethod::ByteCopy
        ));
        assert_eq!(
            fs::read_to_string(&destination).unwrap(),
            "clone-aware copy"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn byte_copy_streams_large_file_with_bounded_buffer() {
        let root = unique_temp_dir("gfm-ops-byte-copy-stream");
        let source = root.join("source.bin");
        let destination = root.join("destination.bin");
        let mut bytes = Vec::with_capacity((COPY_BUFFER_BYTES * 2) + 17);
        for index in 0..((COPY_BUFFER_BYTES * 2) + 17) {
            bytes.push((index % 251) as u8);
        }
        fs::write(&source, &bytes).unwrap();

        let copied = copy_file_bytes(&source, &destination).unwrap();

        assert_eq!(copied, bytes.len() as u64);
        assert_eq!(fs::read(&destination).unwrap(), bytes);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn byte_copy_preserves_mixed_zero_and_nonzero_runs() {
        let root = unique_temp_dir("gfm-ops-byte-copy-zero-runs");
        let source = root.join("source.bin");
        let destination = root.join("destination.bin");
        let mut bytes = vec![0_u8; COPY_BUFFER_BYTES + 41];
        bytes[0] = 1;
        bytes[17] = 2;
        bytes[COPY_BUFFER_BYTES - 1] = 3;
        bytes[COPY_BUFFER_BYTES] = 4;
        bytes[COPY_BUFFER_BYTES + 40] = 5;
        fs::write(&source, &bytes).unwrap();

        let copied = copy_file_bytes(&source, &destination).unwrap();

        assert_eq!(copied, bytes.len() as u64);
        assert_eq!(fs::read(&destination).unwrap(), bytes);
        assert_eq!(
            fs::metadata(&destination).unwrap().len(),
            bytes.len() as u64
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn sparse_metadata_detects_zero_block_holes() {
        use std::os::unix::fs::MetadataExt;

        let root = unique_temp_dir("gfm-ops-sparse-metadata");
        let source = root.join("source.bin");
        let logical_len = COPY_BUFFER_BYTES as u64 * 4;
        {
            let file = File::create(&source).unwrap();
            file.set_len(logical_len).unwrap();
        }

        let metadata = fs::metadata(&source).unwrap();
        if metadata.blocks().saturating_mul(512) >= metadata.len() {
            fs::remove_dir_all(root).unwrap();
            return;
        }

        assert!(metadata_has_sparse_holes(&metadata));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn byte_copy_preserves_sparse_holes_when_host_reports_blocks() {
        use std::os::unix::fs::MetadataExt;

        let root = unique_temp_dir("gfm-ops-byte-copy-sparse");
        let source = root.join("source.bin");
        let destination = root.join("destination.bin");
        let logical_len = (COPY_BUFFER_BYTES as u64 * 8) + 13;
        {
            let mut file = File::create(&source).unwrap();
            file.write_all(b"head").unwrap();
            file.seek(SeekFrom::Start(logical_len - 4)).unwrap();
            file.write_all(b"tail").unwrap();
        }
        let source_metadata = fs::metadata(&source).unwrap();
        if source_metadata.blocks() * 512 >= source_metadata.len() {
            fs::remove_dir_all(root).unwrap();
            return;
        }

        let copied = copy_file_bytes(&source, &destination).unwrap();

        let destination_metadata = fs::metadata(&destination).unwrap();
        assert_eq!(copied, logical_len);
        assert_eq!(destination_metadata.len(), logical_len);
        assert_eq!(fs::read(&destination).unwrap(), fs::read(&source).unwrap());
        assert!(
            destination_metadata.blocks() <= source_metadata.blocks() + 8,
            "expected sparse destination blocks <= source blocks plus tolerance, source={} destination={}",
            source_metadata.blocks(),
            destination_metadata.blocks()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn byte_copy_reports_chunk_progress() {
        let root = unique_temp_dir("gfm-ops-byte-copy-progress");
        let source = root.join("source.bin");
        let destination = root.join("destination.bin");
        let bytes = vec![7_u8; (COPY_BUFFER_BYTES * 2) + 19];
        fs::write(&source, &bytes).unwrap();
        let cancellation = OperationCancellation::default();
        let pause = OperationPause::default();
        let plan = OperationProgress {
            total_items: 1,
            total_bytes: bytes.len() as u64,
            completed_items: 0,
            completed_bytes: 0,
        };
        let mut events = Vec::new();
        let mut callback = |event| events.push(event);
        let mut tracker = ProgressTracker::new(plan, &cancellation, &pause, &mut callback);

        let copied = copy_file_bytes_tracked(
            &source,
            &destination,
            &OperationVolumeCopyPolicy::default(),
            &mut tracker,
        )
        .unwrap();
        tracker.finish_current_item().unwrap();

        assert_eq!(copied, bytes.len() as u64);
        assert!(events
            .iter()
            .any(|event| event.phase == OperationProgressPhase::Advanced
                && event.progress.completed_items == 0
                && event.progress.completed_bytes == COPY_BUFFER_BYTES as u64
                && event.throughput.is_some()));
        let last = events.last().unwrap();
        assert_eq!(last.progress.completed_items, 1);
        assert_eq!(last.progress.completed_bytes, bytes.len() as u64);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn throughput_snapshot_classifies_slow_and_constrained_transfers() {
        let slow = OperationThroughputSnapshot::classify(8 * 1024 * 1024, 1_000_000_000).unwrap();
        let constrained =
            OperationThroughputSnapshot::classify(32 * 1024 * 1024, 1_000_000_000).unwrap();
        let full_speed =
            OperationThroughputSnapshot::classify(256 * 1024 * 1024, 1_000_000_000).unwrap();

        assert_eq!(slow.class, OperationThroughputClass::Slow);
        assert_eq!(constrained.class, OperationThroughputClass::Constrained);
        assert_eq!(full_speed.class, OperationThroughputClass::FullSpeed);
        assert_eq!(OperationThroughputSnapshot::classify(0, 1_000), None);
    }

    #[test]
    fn byte_copy_uses_slow_volume_checkpoint_chunks() {
        let root = unique_temp_dir("gfm-ops-byte-copy-slow-volume");
        let source_root = root.join("network-source");
        let destination_root = root.join("network-destination");
        let source = source_root.join("source.bin");
        let destination = destination_root.join("destination.bin");
        fs::create_dir_all(&source_root).unwrap();
        fs::create_dir_all(&destination_root).unwrap();
        let bytes = vec![5_u8; SLOW_COPY_BUFFER_BYTES + 11];
        fs::write(&source, &bytes).unwrap();
        let policy = OperationVolumeCopyPolicy::default()
            .with_root(&source_root, OperationVolumeClass::Network)
            .with_root(&destination_root, OperationVolumeClass::Network);
        let cancellation = OperationCancellation::default();
        let pause = OperationPause::default();
        let plan = OperationProgress {
            total_items: 1,
            total_bytes: bytes.len() as u64,
            completed_items: 0,
            completed_bytes: 0,
        };
        let mut events = Vec::new();
        let mut callback = |event| events.push(event);
        let mut tracker = ProgressTracker::new(plan, &cancellation, &pause, &mut callback);

        let copied = copy_file_bytes_tracked(&source, &destination, &policy, &mut tracker).unwrap();
        tracker.finish_current_item().unwrap();

        assert_eq!(copied, bytes.len() as u64);
        assert_eq!(fs::read(&destination).unwrap(), bytes);
        assert!(events
            .iter()
            .any(|event| event.phase == OperationProgressPhase::Advanced
                && event.progress.completed_items == 0
                && event.progress.completed_bytes == SLOW_COPY_BUFFER_BYTES as u64));
        assert!(!events
            .iter()
            .any(|event| event.progress.completed_bytes == COPY_BUFFER_BYTES as u64));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn byte_copy_cancellation_removes_partial_destination() {
        let root = unique_temp_dir("gfm-ops-byte-copy-cancel");
        let source = root.join("source.bin");
        let destination = root.join("destination.bin");
        let bytes = vec![9_u8; (COPY_BUFFER_BYTES * 2) + 5];
        fs::write(&source, &bytes).unwrap();
        let cancellation = OperationCancellation::default();
        let cancellation_callback = cancellation.clone();
        let pause = OperationPause::default();
        let plan = OperationProgress {
            total_items: 1,
            total_bytes: bytes.len() as u64,
            completed_items: 0,
            completed_bytes: 0,
        };
        let mut events = Vec::new();
        let mut callback = |event: OperationProgressEvent| {
            events.push(event);
            if event.phase == OperationProgressPhase::Advanced && event.progress.completed_bytes > 0
            {
                cancellation_callback.cancel();
            }
        };
        let mut tracker = ProgressTracker::new(plan, &cancellation, &pause, &mut callback);

        let err = copy_file_bytes_tracked(
            &source,
            &destination,
            &OperationVolumeCopyPolicy::default(),
            &mut tracker,
        )
        .unwrap_err();

        assert!(matches!(err, GfmError::Cancelled));
        assert!(!destination.exists());
        assert!(events
            .iter()
            .any(|event| event.phase == OperationProgressPhase::Advanced
                && event.progress.completed_bytes == COPY_BUFFER_BYTES as u64));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn byte_copy_refuses_existing_destination() {
        let root = unique_temp_dir("gfm-ops-byte-copy-existing");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "fresh").unwrap();
        fs::write(&destination, "existing").unwrap();

        let err = copy_file_bytes(&source, &destination).unwrap_err();

        assert!(
            matches!(err, GfmError::Io { .. }),
            "expected io conflict, got {err:?}"
        );
        assert_eq!(fs::read_to_string(&destination).unwrap(), "existing");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_verification_rejects_size_mismatch() {
        let root = unique_temp_dir("gfm-ops-verify-size");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "complete").unwrap();
        fs::write(&destination, "short").unwrap();

        let err = verify_copy(&source, &destination, VerificationPolicy::Size).unwrap_err();

        assert!(err.to_string().contains("source size"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_verification_rejects_byte_mismatch() {
        let root = unique_temp_dir("gfm-ops-verify-bytes");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "same length").unwrap();
        fs::write(&destination, "same Length").unwrap();

        let err = verify_copy(&source, &destination, VerificationPolicy::Bytes).unwrap_err();

        assert!(err.to_string().contains("byte mismatch"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_context_defaults_to_size_verification_for_fast_local_copies() {
        let root = unique_temp_dir("gfm-ops-default-verify-size");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "default verification").unwrap();
        let context = OperationContext::new(&journal);

        assert_eq!(context.verification, VerificationPolicy::Size);
        Operator::new(context)
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        assert_eq!(
            fs::read_to_string(&destination).unwrap(),
            "default verification"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_can_use_size_only_verification_policy() {
        let root = unique_temp_dir("gfm-ops-verify-size-policy");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "policy").unwrap();

        Operator::new(OperationContext::new(&journal).with_verification(VerificationPolicy::Size))
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        assert_eq!(fs::read_to_string(&destination).unwrap(), "policy");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_can_opt_into_byte_verification_policy() {
        let root = unique_temp_dir("gfm-ops-verify-bytes-policy");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "byte policy").unwrap();

        Operator::new(OperationContext::new(&journal).with_verification(VerificationPolicy::Bytes))
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        assert_eq!(fs::read_to_string(&destination).unwrap(), "byte policy");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn copy_file_uses_apfs_clone_when_host_supports_it() {
        let root = unique_temp_dir("gfm-ops-apfs-clone");
        let source = root.join("source.bin");
        let probe = root.join("probe.bin");
        let destination = root.join("destination.bin");
        fs::write(&source, b"copy-on-write candidate").unwrap();

        match clone_file(&source, &probe) {
            Ok(()) => {
                fs::remove_file(&probe).unwrap();
                let method = copy_file(&source, &destination, VerificationPolicy::Bytes).unwrap();
                assert_eq!(method, CopyMethod::ApfsClone);
                assert_eq!(fs::read(&destination).unwrap(), b"copy-on-write candidate");
            }
            Err(err) if clone_fallback_allowed(&err) => {
                let method = copy_file(&source, &destination, VerificationPolicy::Bytes).unwrap();
                assert_eq!(method, CopyMethod::ByteCopy);
            }
            Err(err) => panic!("unexpected clonefile failure: {err}"),
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_preserves_xattrs_when_host_supports_them() {
        let root = unique_temp_dir("gfm-ops-xattrs");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "finder metadata").unwrap();
        match xattr::set(&source, "user.gfm.test", b"tagged") {
            Ok(()) => {}
            Err(err) if xattr_copy_unsupported(&err) => {
                fs::remove_dir_all(root).unwrap();
                return;
            }
            Err(err) => panic!("unexpected xattr setup failure: {err}"),
        }

        Operator::new(OperationContext::new(&journal))
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        assert_eq!(
            xattr::get(&destination, "user.gfm.test")
                .unwrap()
                .as_deref(),
            Some(b"tagged".as_slice())
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_preserves_modified_time() {
        let root = unique_temp_dir("gfm-ops-times");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "dated").unwrap();
        let expected = filetime::FileTime::from_unix_time(1_700_000_000, 123_000_000);
        filetime::set_file_mtime(&source, expected).unwrap();

        Operator::new(OperationContext::new(&journal))
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        let copied = fs::metadata(&destination).unwrap();
        assert_eq!(
            filetime::FileTime::from_last_modification_time(&copied),
            expected
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_vendor = "apple")]
    #[test]
    fn copy_preserves_birthtime_when_host_supports_it() {
        use std::os::darwin::fs::FileTimesExt;
        use std::time::{Duration, UNIX_EPOCH};

        let root = unique_temp_dir("gfm-ops-birthtime");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "created").unwrap();
        let created = UNIX_EPOCH + Duration::from_secs(1_600_000_123);
        let file = File::open(&source).unwrap();
        match file.set_times(fs::FileTimes::new().set_created(created)) {
            Ok(()) => {}
            Err(err) if time_preservation_unsupported(&err) => {
                fs::remove_dir_all(root).unwrap();
                return;
            }
            Err(err) => panic!("unexpected birthtime setup failure: {err}"),
        }

        Operator::new(OperationContext::new(&journal))
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        assert_eq!(
            fs::metadata(&destination).unwrap().created().unwrap(),
            created
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_vendor = "apple")]
    #[test]
    fn copy_preserves_bsd_file_flags_when_host_supports_them() {
        use nix::sys::stat::FileFlag;
        use std::os::darwin::fs::MetadataExt;

        let root = unique_temp_dir("gfm-ops-bsd-flags");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "flags").unwrap();
        let flags = FileFlag::UF_HIDDEN;
        match nix::unistd::chflags(&source, flags) {
            Ok(()) => {}
            Err(err) => {
                let err = io::Error::from_raw_os_error(err as i32);
                if file_flag_preservation_unsupported(&err) {
                    fs::remove_dir_all(root).unwrap();
                    return;
                }
                panic!("unexpected file flag setup failure: {err}");
            }
        }

        Operator::new(OperationContext::new(&journal))
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        let copied_flags = fs::metadata(&destination).unwrap().st_flags();
        assert_eq!(copied_flags & flags.bits(), flags.bits());

        nix::unistd::chflags(&source, FileFlag::empty()).unwrap();
        nix::unistd::chflags(&destination, FileFlag::empty()).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn copy_preserves_access_control_lists_when_host_supports_them() {
        let root = unique_temp_dir("gfm-ops-acls");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "acl").unwrap();
        let Ok(user) = std::env::var("USER") else {
            fs::remove_dir_all(root).unwrap();
            return;
        };
        if user.is_empty() {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        let entries = vec![exacl::AclEntry::allow_user(
            &user,
            exacl::Perm::READ | exacl::Perm::READATTR | exacl::Perm::READSECURITY,
            None,
        )];
        let source_paths = [&source];
        match exacl::setfacl(&source_paths, &entries, None::<exacl::AclOption>) {
            Ok(()) => {}
            Err(err) if acl_copy_unsupported(&err) => {
                fs::remove_dir_all(root).unwrap();
                return;
            }
            Err(err) => panic!("unexpected acl setup failure: {err}"),
        }

        Operator::new(OperationContext::new(&journal))
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        assert_eq!(
            exacl::getfacl(&destination, None::<exacl::AclOption>).unwrap(),
            exacl::getfacl(&source, None::<exacl::AclOption>).unwrap()
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn copy_preserves_owner_and_group_when_host_allows_it() {
        use std::os::unix::fs::MetadataExt;

        let root = unique_temp_dir("gfm-ops-ownership");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "ownership").unwrap();

        Operator::new(OperationContext::new(&journal))
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        let source_metadata = fs::symlink_metadata(&source).unwrap();
        let destination_metadata = fs::symlink_metadata(&destination).unwrap();
        assert_eq!(destination_metadata.uid(), source_metadata.uid());
        assert_eq!(destination_metadata.gid(), source_metadata.gid());

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn copy_preserves_symlink_instead_of_copying_target() {
        let root = unique_temp_dir("gfm-ops-symlink");
        let journal = root.join("journal.log");
        let target = root.join("target.txt");
        let source = root.join("source-link");
        let destination = root.join("destination-link");
        fs::write(&target, "target bytes").unwrap();
        std::os::unix::fs::symlink(&target, &source).unwrap();

        Operator::new(OperationContext::new(&journal))
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        let destination_metadata = fs::symlink_metadata(&destination).unwrap();
        assert!(destination_metadata.file_type().is_symlink());
        assert_eq!(fs::read_link(&destination).unwrap(), target);

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn copy_replace_symlink_uses_staged_destination() {
        let root = unique_temp_dir("gfm-ops-copy-replace-symlink");
        let journal = root.join("journal.log");
        let old_target = root.join("old-target.txt");
        let new_target = root.join("new-target.txt");
        let source = root.join("source-link");
        let destination = root.join("destination-link");
        fs::write(&old_target, "old").unwrap();
        fs::write(&new_target, "new").unwrap();
        std::os::unix::fs::symlink(&new_target, &source).unwrap();
        std::os::unix::fs::symlink(&old_target, &destination).unwrap();

        Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Replace))
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        assert_eq!(fs::read_link(&source).unwrap(), new_target);
        assert_eq!(fs::read_link(&destination).unwrap(), new_target);
        let leaked_stage = fs::read_dir(&root)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains(".gfm-replace-")
            });
        assert!(!leaked_stage);

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn cancelled_copy_replace_symlink_preserves_existing_destination() {
        let root = unique_temp_dir("gfm-ops-copy-replace-symlink-cancel");
        let journal = root.join("journal.log");
        let old_target = root.join("old-target.txt");
        let new_target = root.join("new-target.txt");
        let source = root.join("source-link");
        let destination = root.join("destination-link");
        fs::write(&old_target, "old").unwrap();
        fs::write(&new_target, "new").unwrap();
        std::os::unix::fs::symlink(&new_target, &source).unwrap();
        std::os::unix::fs::symlink(&old_target, &destination).unwrap();
        let cancellation = OperationCancellation::default();
        let operator = Operator::new(
            OperationContext::new(&journal)
                .with_conflict(ConflictPolicy::Replace)
                .with_cancellation(cancellation.clone()),
        );

        let err = operator
            .execute_with_progress(
                Operation::Copy {
                    from: source.clone(),
                    to: destination.clone(),
                },
                |event| {
                    if event.phase == OperationProgressPhase::Planned {
                        cancellation.cancel();
                    }
                },
            )
            .unwrap_err();

        assert!(matches!(err, GfmError::Cancelled));
        assert_eq!(fs::read_link(&source).unwrap(), new_target);
        assert_eq!(fs::read_link(&destination).unwrap(), old_target);

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn copy_preserves_symlink_timestamps_when_host_supports_them() {
        let root = unique_temp_dir("gfm-ops-symlink-times");
        let journal = root.join("journal.log");
        let target = root.join("target.txt");
        let source = root.join("source-link");
        let destination = root.join("destination-link");
        fs::write(&target, "target bytes").unwrap();
        std::os::unix::fs::symlink(&target, &source).unwrap();
        let atime = filetime::FileTime::from_unix_time(1_650_000_000, 111_000_000);
        let mtime = filetime::FileTime::from_unix_time(1_650_000_123, 222_000_000);
        match filetime::set_symlink_file_times(&source, atime, mtime) {
            Ok(()) => {}
            Err(err) if time_preservation_unsupported(&err) => {
                fs::remove_dir_all(root).unwrap();
                return;
            }
            Err(err) => panic!("unexpected symlink time setup failure: {err}"),
        }

        Operator::new(OperationContext::new(&journal))
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        assert_eq!(
            filetime::FileTime::from_last_modification_time(
                &fs::symlink_metadata(&destination).unwrap()
            ),
            mtime
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn recursive_copy_preserves_hard_link_topology() {
        use std::os::unix::fs::MetadataExt;

        let root = unique_temp_dir("gfm-ops-hard-links");
        let journal = root.join("journal.log");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(source.join("nested")).unwrap();
        let original = source.join("original.txt");
        let alias = source.join("nested").join("alias.txt");
        fs::write(&original, "shared inode").unwrap();
        match fs::hard_link(&original, &alias) {
            Ok(()) => {}
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::Unsupported | io::ErrorKind::PermissionDenied
                ) =>
            {
                fs::remove_dir_all(root).unwrap();
                return;
            }
            Err(err) => panic!("unexpected hard-link setup failure: {err}"),
        }

        Operator::new(OperationContext::new(&journal))
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        let copied_original = destination.join("original.txt");
        let copied_alias = destination.join("nested").join("alias.txt");
        assert_eq!(fs::read_to_string(&copied_alias).unwrap(), "shared inode");
        let original_metadata = fs::metadata(&copied_original).unwrap();
        let alias_metadata = fs::metadata(&copied_alias).unwrap();
        assert_eq!(original_metadata.dev(), alias_metadata.dev());
        assert_eq!(original_metadata.ino(), alias_metadata.ino());
        assert!(original_metadata.nlink() >= 2);

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn copy_directory_preserves_nested_symlink() {
        let root = unique_temp_dir("gfm-ops-directory-symlink");
        let journal = root.join("journal.log");
        let source = root.join("source");
        let destination = root.join("destination");
        let target = root.join("outside-target.txt");
        fs::create_dir_all(&source).unwrap();
        fs::write(&target, "outside").unwrap();
        std::os::unix::fs::symlink(&target, source.join("link")).unwrap();

        Operator::new(OperationContext::new(&journal))
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        let copied_link = destination.join("link");
        assert!(fs::symlink_metadata(&copied_link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_link(copied_link).unwrap(), target);

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn copy_directory_applies_readonly_permissions_after_children() {
        use std::os::unix::fs::PermissionsExt;

        let root = unique_temp_dir("gfm-ops-readonly-directory");
        let journal = root.join("journal.log");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("nested").join("file.txt"), "nested").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o555)).unwrap();

        Operator::new(OperationContext::new(&journal))
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        assert_eq!(
            fs::read_to_string(destination.join("nested").join("file.txt")).unwrap(),
            "nested"
        );
        assert_eq!(
            fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
            0o555
        );

        fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o755)).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_replace_directory_uses_staged_destination() {
        let root = unique_temp_dir("gfm-ops-copy-replace-directory");
        let journal = root.join("journal.log");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("nested").join("new.txt"), "new").unwrap();
        fs::create_dir_all(destination.join("nested")).unwrap();
        fs::write(destination.join("nested").join("old.txt"), "old").unwrap();

        Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Replace))
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        assert_eq!(
            fs::read_to_string(destination.join("nested").join("new.txt")).unwrap(),
            "new"
        );
        assert!(!destination.join("nested").join("old.txt").exists());
        assert_eq!(
            fs::read_to_string(source.join("nested").join("new.txt")).unwrap(),
            "new"
        );
        let leaked_replace_sibling = fs::read_dir(&root)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .any(|entry| entry.file_name().to_string_lossy().contains(".gfm-replace"));
        assert!(!leaked_replace_sibling);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelled_copy_replace_directory_preserves_existing_destination() {
        let root = unique_temp_dir("gfm-ops-copy-replace-directory-cancel");
        let journal = root.join("journal.log");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("nested").join("new.txt"), "new").unwrap();
        fs::create_dir_all(destination.join("nested")).unwrap();
        fs::write(destination.join("nested").join("old.txt"), "old").unwrap();
        let cancellation = OperationCancellation::default();
        let cancellation_callback = cancellation.clone();

        let err = Operator::new(
            OperationContext::new(&journal)
                .with_conflict(ConflictPolicy::Replace)
                .with_cancellation(cancellation),
        )
        .execute_with_progress(
            Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            },
            |event| {
                if event.phase == OperationProgressPhase::Advanced
                    && event.progress.completed_items == 1
                {
                    cancellation_callback.cancel();
                }
            },
        )
        .unwrap_err();

        assert!(matches!(err, GfmError::Cancelled));
        assert_eq!(
            fs::read_to_string(destination.join("nested").join("old.txt")).unwrap(),
            "old"
        );
        assert!(!destination.join("nested").join("new.txt").exists());
        let leaked_replace_sibling = fs::read_dir(&root)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .any(|entry| entry.file_name().to_string_lossy().contains(".gfm-replace"));
        assert!(!leaked_replace_sibling);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fails_on_destination_conflict_without_mutating_source() {
        let root = unique_temp_dir("gfm-ops-conflict");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "source").unwrap();
        fs::write(&destination, "destination").unwrap();

        let operator = Operator::new(OperationContext::new(&journal));
        let err = operator
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap_err();

        assert!(matches!(err, GfmError::Conflict { .. }));
        assert_eq!(fs::read_to_string(&source).unwrap(), "source");
        assert_eq!(fs::read_to_string(&destination).unwrap(), "destination");
        let journal_entries = operator.journal().unwrap();
        assert_eq!(journal_entries.len(), 2);
        assert_eq!(journal_entries[1].status, OperationStatus::Failed);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_replace_regular_file_uses_staged_destination() {
        let root = unique_temp_dir("gfm-ops-copy-replace-staged");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "new destination bytes").unwrap();
        fs::write(&destination, "old destination bytes").unwrap();

        Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Replace))
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        assert_eq!(
            fs::read_to_string(&destination).unwrap(),
            "new destination bytes"
        );
        assert_eq!(
            fs::read_to_string(&source).unwrap(),
            "new destination bytes"
        );
        let leaked_stage = fs::read_dir(&root)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains(".gfm-replace-")
            });
        assert!(!leaked_stage);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelled_copy_replace_regular_file_preserves_existing_destination() {
        let root = unique_temp_dir("gfm-ops-copy-replace-cancel");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "new destination bytes").unwrap();
        fs::write(&destination, "old destination bytes").unwrap();
        let cancellation = OperationCancellation::default();
        let operator = Operator::new(
            OperationContext::new(&journal)
                .with_conflict(ConflictPolicy::Replace)
                .with_cancellation(cancellation.clone()),
        );

        let err = operator
            .execute_with_progress(
                Operation::Copy {
                    from: source.clone(),
                    to: destination.clone(),
                },
                |event| {
                    if event.phase == OperationProgressPhase::Planned {
                        cancellation.cancel();
                    }
                },
            )
            .unwrap_err();

        assert!(matches!(err, GfmError::Cancelled));
        assert_eq!(
            fs::read_to_string(&destination).unwrap(),
            "old destination bytes"
        );
        assert_eq!(
            fs::read_to_string(&source).unwrap(),
            "new destination bytes"
        );
        let leaked_stage = fs::read_dir(&root)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains(".gfm-replace-")
            });
        assert!(!leaked_stage);

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn copy_replace_same_inode_is_noop() {
        use std::os::unix::fs::MetadataExt;

        let root = unique_temp_dir("gfm-ops-copy-replace-same-inode");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "shared inode").unwrap();
        fs::hard_link(&source, &destination).unwrap();
        let before_source = fs::metadata(&source).unwrap();
        let before_destination = fs::metadata(&destination).unwrap();

        Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Replace))
            .execute(Operation::Copy {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        let after_source = fs::metadata(&source).unwrap();
        let after_destination = fs::metadata(&destination).unwrap();
        assert_eq!(fs::read_to_string(&destination).unwrap(), "shared inode");
        assert_eq!(before_source.ino(), before_destination.ino());
        assert_eq!(after_source.ino(), after_destination.ino());
        assert_eq!(before_source.ino(), after_source.ino());
        assert!(after_source.nlink() >= 2);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_skip_conflict_journals_skipped_without_mutation() {
        let root = unique_temp_dir("gfm-ops-skip-copy");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "source").unwrap();
        fs::write(&destination, "destination").unwrap();
        let mut events = Vec::new();

        let entry =
            Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Skip))
                .execute_with_progress(
                    Operation::Copy {
                        from: source.clone(),
                        to: destination.clone(),
                    },
                    |event| events.push(event),
                )
                .unwrap();

        assert_eq!(entry.status, OperationStatus::Skipped);
        assert!(events.is_empty());
        assert_eq!(fs::read_to_string(&source).unwrap(), "source");
        assert_eq!(fs::read_to_string(&destination).unwrap(), "destination");
        let journal_entries = read_journal(&journal).unwrap();
        assert_eq!(journal_entries.len(), 2);
        assert_eq!(journal_entries[0].status, OperationStatus::Started);
        assert_eq!(journal_entries[1].status, OperationStatus::Skipped);
        assert_eq!(
            journal_entries[1].message.as_deref(),
            Some("operation skipped by conflict policy")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_ignores_skipped_operations_as_terminal() {
        let root = unique_temp_dir("gfm-ops-recover-skipped");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "source").unwrap();
        fs::write(&destination, "destination").unwrap();
        let operation = Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        };
        append_journal(&journal, &JournalEntry::started(48, operation.clone())).unwrap();
        append_journal(&journal, &JournalEntry::skipped(48, operation)).unwrap();

        let report = Operator::new(OperationContext::new(&journal))
            .recover_with_policy(OperationRecoveryPolicy {
                retry_failed: true,
                max_attempts: 2,
            })
            .unwrap();

        assert!(report.outcomes.is_empty());
        assert_eq!(fs::read_to_string(&destination).unwrap(), "destination");
        assert_eq!(read_journal(&journal).unwrap().len(), 2);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn batch_conflict_plan_applies_default_policy_to_all_targets() {
        let root = unique_temp_dir("gfm-ops-batch-apply-all");
        let journal = root.join("journal.log");
        let first_source = root.join("first-source.txt");
        let first_destination = root.join("first-destination.txt");
        let second_source = root.join("second-source.txt");
        let second_destination = root.join("second-destination.txt");
        fs::write(&first_source, "new first").unwrap();
        fs::write(&first_destination, "old first").unwrap();
        fs::write(&second_source, "new second").unwrap();
        fs::write(&second_destination, "old second").unwrap();

        let report = Operator::new(OperationContext::new(&journal))
            .execute_batch_with_conflicts(
                vec![
                    Operation::Copy {
                        from: first_source.clone(),
                        to: first_destination.clone(),
                    },
                    Operation::Copy {
                        from: second_source.clone(),
                        to: second_destination.clone(),
                    },
                ],
                OperationConflictPlan::new(ConflictPolicy::Skip),
            )
            .unwrap();

        assert_eq!(report.outcomes.len(), 2);
        assert!(report
            .outcomes
            .iter()
            .all(|outcome| outcome.conflict == ConflictPolicy::Skip
                && outcome.status == OperationStatus::Skipped));
        assert_eq!(fs::read_to_string(&first_destination).unwrap(), "old first");
        assert_eq!(
            fs::read_to_string(&second_destination).unwrap(),
            "old second"
        );
        let entries = read_journal(&journal).unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.status == OperationStatus::Skipped)
                .count(),
            2
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn batch_conflict_plan_uses_per_target_override() {
        let root = unique_temp_dir("gfm-ops-batch-per-target");
        let journal = root.join("journal.log");
        let replace_source = root.join("replace-source.txt");
        let replace_destination = root.join("replace-destination.txt");
        let skip_source = root.join("skip-source.txt");
        let skip_destination = root.join("skip-destination.txt");
        fs::write(&replace_source, "new replace").unwrap();
        fs::write(&replace_destination, "old replace").unwrap();
        fs::write(&skip_source, "new skip").unwrap();
        fs::write(&skip_destination, "old skip").unwrap();

        let report = Operator::new(OperationContext::new(&journal))
            .execute_batch_with_conflicts(
                vec![
                    Operation::Copy {
                        from: replace_source.clone(),
                        to: replace_destination.clone(),
                    },
                    Operation::Copy {
                        from: skip_source.clone(),
                        to: skip_destination.clone(),
                    },
                ],
                OperationConflictPlan::new(ConflictPolicy::Skip)
                    .with_target(&replace_destination, ConflictPolicy::Replace),
            )
            .unwrap();

        assert_eq!(report.outcomes.len(), 2);
        assert_eq!(report.outcomes[0].conflict, ConflictPolicy::Replace);
        assert_eq!(report.outcomes[0].status, OperationStatus::Completed);
        assert_eq!(report.outcomes[1].conflict, ConflictPolicy::Skip);
        assert_eq!(report.outcomes[1].status, OperationStatus::Skipped);
        assert_eq!(
            fs::read_to_string(&replace_destination).unwrap(),
            "new replace"
        );
        assert_eq!(fs::read_to_string(&skip_destination).unwrap(), "old skip");
        let entries = read_journal(&journal).unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[1].status, OperationStatus::Completed);
        assert_eq!(entries[3].status, OperationStatus::Skipped);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn merge_conflict_treats_finder_packages_as_atomic_items() {
        let root = unique_temp_dir("gfm-ops-package-merge");
        let journal = root.join("journal.log");
        let source = root.join("Demo.app");
        let destination = root.join("Demo Copy.app");
        fs::create_dir_all(source.join("Contents")).unwrap();
        fs::create_dir_all(destination.join("Contents")).unwrap();
        fs::write(source.join("Contents").join("new.txt"), "source").unwrap();
        fs::write(
            destination.join("Contents").join("existing.txt"),
            "destination",
        )
        .unwrap();

        let err =
            Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Merge))
                .execute(Operation::Copy {
                    from: source.clone(),
                    to: destination.clone(),
                })
                .unwrap_err();

        assert!(matches!(err, GfmError::Conflict { .. }));
        assert!(!destination.join("Contents").join("new.txt").exists());
        assert_eq!(
            fs::read_to_string(destination.join("Contents").join("existing.txt")).unwrap(),
            "destination"
        );
        let entries = read_journal(&journal).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].status, OperationStatus::Started);
        assert_eq!(entries[1].status, OperationStatus::Failed);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelled_fresh_directory_copy_removes_incomplete_destination() {
        let root = unique_temp_dir("gfm-ops-directory-cancel");
        let journal = root.join("journal.log");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("nested").join("file.txt"), "nested").unwrap();
        let cancellation = OperationCancellation::default();
        let cancellation_callback = cancellation.clone();

        let err = Operator::new(OperationContext::new(&journal).with_cancellation(cancellation))
            .execute_with_progress(
                Operation::Copy {
                    from: source.clone(),
                    to: destination.clone(),
                },
                |event| {
                    if event.phase == OperationProgressPhase::Advanced
                        && event.progress.completed_items == 1
                    {
                        cancellation_callback.cancel();
                    }
                },
            )
            .unwrap_err();

        assert!(matches!(err, GfmError::Cancelled));
        assert!(!path_exists_or_symlink(&destination));
        let entries = read_journal(&journal).unwrap();
        assert_eq!(entries[1].status, OperationStatus::Cancelled);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelled_fresh_package_copy_removes_incomplete_bundle() {
        let root = unique_temp_dir("gfm-ops-package-cancel");
        let journal = root.join("journal.log");
        let source = root.join("Demo.app");
        let destination = root.join("Demo Copy.app");
        fs::create_dir_all(source.join("Contents").join("Resources")).unwrap();
        fs::write(source.join("Contents").join("Info.plist"), "plist").unwrap();
        fs::write(
            source.join("Contents").join("Resources").join("asset.txt"),
            "asset",
        )
        .unwrap();
        let cancellation = OperationCancellation::default();
        let cancellation_callback = cancellation.clone();

        let err = Operator::new(OperationContext::new(&journal).with_cancellation(cancellation))
            .execute_with_progress(
                Operation::Copy {
                    from: source.clone(),
                    to: destination.clone(),
                },
                |event| {
                    if event.phase == OperationProgressPhase::Advanced
                        && event.progress.completed_items == 1
                    {
                        cancellation_callback.cancel();
                    }
                },
            )
            .unwrap_err();

        assert!(matches!(err, GfmError::Cancelled));
        assert!(!path_exists_or_symlink(&destination));
        let entries = read_journal(&journal).unwrap();
        assert_eq!(entries[1].status, OperationStatus::Cancelled);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_keep_both_allocates_finder_style_destination() {
        let root = unique_temp_dir("gfm-ops-keep-both-copy");
        let journal = root.join("journal.log");
        let source = root.join("report.md");
        let destination = root.join("destination.md");
        let copy_destination = root.join("destination copy.md");
        fs::write(&source, "source").unwrap();
        fs::write(&destination, "destination").unwrap();

        let entry =
            Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::KeepBoth))
                .execute(Operation::Copy {
                    from: source.clone(),
                    to: destination.clone(),
                })
                .unwrap();

        assert_eq!(fs::read_to_string(&source).unwrap(), "source");
        assert_eq!(fs::read_to_string(&destination).unwrap(), "destination");
        assert_eq!(fs::read_to_string(&copy_destination).unwrap(), "source");
        assert_eq!(
            entry.operation,
            Operation::Copy {
                from: source,
                to: copy_destination
            }
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn move_keep_both_allocates_next_available_destination() {
        let root = unique_temp_dir("gfm-ops-keep-both-move");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        let first_copy = root.join("destination copy.txt");
        let second_copy = root.join("destination copy 2.txt");
        fs::write(&source, "new").unwrap();
        fs::write(&destination, "old").unwrap();
        fs::write(&first_copy, "older").unwrap();

        let entry =
            Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::KeepBoth))
                .execute(Operation::Move {
                    from: source.clone(),
                    to: destination.clone(),
                })
                .unwrap();

        assert!(!source.exists());
        assert_eq!(fs::read_to_string(&destination).unwrap(), "old");
        assert_eq!(fs::read_to_string(&first_copy).unwrap(), "older");
        assert_eq!(fs::read_to_string(&second_copy).unwrap(), "new");
        assert_eq!(
            entry.operation,
            Operation::Move {
                from: source,
                to: second_copy
            }
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_merge_combines_directories_without_overwriting_existing_files() {
        let root = unique_temp_dir("gfm-ops-merge-copy");
        let journal = root.join("journal.log");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::create_dir_all(destination.join("nested")).unwrap();
        fs::write(source.join("nested").join("new.txt"), "new").unwrap();
        fs::write(destination.join("nested").join("old.txt"), "old").unwrap();

        let entry =
            Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Merge))
                .execute(Operation::Copy {
                    from: source.clone(),
                    to: destination.clone(),
                })
                .unwrap();

        assert_eq!(entry.status, OperationStatus::Completed);
        assert_eq!(
            fs::read_to_string(destination.join("nested").join("new.txt")).unwrap(),
            "new"
        );
        assert_eq!(
            fs::read_to_string(destination.join("nested").join("old.txt")).unwrap(),
            "old"
        );
        assert_eq!(
            fs::read_to_string(source.join("nested").join("new.txt")).unwrap(),
            "new"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_merge_rejects_existing_file_conflict_without_overwrite() {
        let root = unique_temp_dir("gfm-ops-merge-conflict");
        let journal = root.join("journal.log");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(source.join("same.txt"), "source").unwrap();
        fs::write(destination.join("same.txt"), "destination").unwrap();

        let err =
            Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Merge))
                .execute(Operation::Copy {
                    from: source,
                    to: destination.clone(),
                })
                .unwrap_err();

        assert!(matches!(err, GfmError::Conflict { .. }));
        assert_eq!(
            fs::read_to_string(destination.join("same.txt")).unwrap(),
            "destination"
        );
        let entries = read_journal(&journal).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].status, OperationStatus::Started);
        assert_eq!(entries[1].status, OperationStatus::Failed);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn move_merge_combines_directories_and_removes_source_after_success() {
        let root = unique_temp_dir("gfm-ops-merge-move");
        let journal = root.join("journal.log");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::create_dir_all(destination.join("nested")).unwrap();
        fs::write(source.join("nested").join("new.txt"), "new").unwrap();
        fs::write(destination.join("nested").join("old.txt"), "old").unwrap();

        Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Merge))
            .execute(Operation::Move {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        assert!(!source.exists());
        assert_eq!(
            fs::read_to_string(destination.join("nested").join("new.txt")).unwrap(),
            "new"
        );
        assert_eq!(
            fs::read_to_string(destination.join("nested").join("old.txt")).unwrap(),
            "old"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn trash_metadata_round_trips_original_restore_destination() {
        let root = unique_temp_dir("gfm-ops-trash-metadata");
        let metadata = root.join("trash.tsv");
        let original = root.join("Documents").join("report.md");
        fs::create_dir_all(original.parent().unwrap()).unwrap();

        append_trash_metadata(&metadata, &original).unwrap();

        let entries = read_trash_metadata(&metadata).unwrap();
        let entry = entries.get("report.md").unwrap();
        assert_eq!(entry.original_path, original);
        assert!(entry.can_restore);
        assert!(entry.can_delete_permanently);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restore_moves_trash_entry_to_metadata_destination_and_removes_metadata() {
        let root = unique_temp_dir("gfm-ops-restore");
        let journal = root.join("journal.log");
        let metadata = root.join("trash.tsv");
        let trash_dir = root.join("Trash");
        let original_dir = root.join("Documents");
        let trashed = trash_dir.join("report.md");
        let original = original_dir.join("report.md");
        fs::create_dir_all(&trash_dir).unwrap();
        fs::create_dir_all(&original_dir).unwrap();
        fs::write(&trashed, "restore me").unwrap();
        append_trash_metadata_entry(
            &metadata,
            &TrashRestoreMetadata {
                name: "report.md".to_string(),
                original_path: original.clone(),
                deleted_at_nanos: 7,
                can_restore: true,
                can_delete_permanently: true,
                permission_issue: None,
            },
        )
        .unwrap();

        let entry = Operator::new(
            OperationContext::new(&journal)
                .with_trash_metadata_path(&metadata)
                .with_conflict(ConflictPolicy::Fail),
        )
        .execute(Operation::Restore {
            from: trashed.clone(),
            to: original.clone(),
        })
        .unwrap();

        assert_eq!(entry.status, OperationStatus::Completed);
        assert!(!trashed.exists());
        assert_eq!(fs::read_to_string(&original).unwrap(), "restore me");
        assert!(read_trash_metadata(&metadata).unwrap().is_empty());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restore_conflict_preserves_trash_entry_and_existing_destination() {
        let root = unique_temp_dir("gfm-ops-restore-conflict");
        let journal = root.join("journal.log");
        let metadata = root.join("trash.tsv");
        let trash_dir = root.join("Trash");
        let original_dir = root.join("Documents");
        let trashed = trash_dir.join("report.md");
        let original = original_dir.join("report.md");
        fs::create_dir_all(&trash_dir).unwrap();
        fs::create_dir_all(&original_dir).unwrap();
        fs::write(&trashed, "trashed").unwrap();
        fs::write(&original, "existing").unwrap();
        append_trash_metadata(&metadata, &original).unwrap();

        let err =
            Operator::new(OperationContext::new(&journal).with_trash_metadata_path(&metadata))
                .execute(Operation::Restore {
                    from: trashed.clone(),
                    to: original.clone(),
                })
                .unwrap_err();

        assert!(matches!(err, GfmError::Conflict { .. }));
        assert_eq!(fs::read_to_string(&trashed).unwrap(), "trashed");
        assert_eq!(fs::read_to_string(&original).unwrap(), "existing");
        assert!(read_trash_metadata(&metadata)
            .unwrap()
            .contains_key("report.md"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn permanent_delete_removes_trash_metadata_after_success() {
        let root = unique_temp_dir("gfm-ops-permanent-delete");
        let journal = root.join("journal.log");
        let metadata = root.join("trash.tsv");
        let trash_dir = root.join("Trash");
        let trashed = trash_dir.join("report.md");
        let original = root.join("Documents").join("report.md");
        fs::create_dir_all(&trash_dir).unwrap();
        fs::write(&trashed, "delete forever").unwrap();
        append_trash_metadata_entry(
            &metadata,
            &TrashRestoreMetadata {
                name: "report.md".to_string(),
                original_path: original,
                deleted_at_nanos: 9,
                can_restore: true,
                can_delete_permanently: true,
                permission_issue: None,
            },
        )
        .unwrap();

        let entry =
            Operator::new(OperationContext::new(&journal).with_trash_metadata_path(&metadata))
                .execute(Operation::Delete {
                    path: trashed.clone(),
                })
                .unwrap();

        assert_eq!(entry.status, OperationStatus::Completed);
        assert!(!trashed.exists());
        assert!(read_trash_metadata(&metadata).unwrap().is_empty());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_permanent_delete_preserves_trash_metadata() {
        let root = unique_temp_dir("gfm-ops-permanent-delete-fail");
        let journal = root.join("journal.log");
        let metadata = root.join("trash.tsv");
        let trashed = root.join("Trash").join("missing.md");
        let original = root.join("Documents").join("missing.md");
        append_trash_metadata_entry(
            &metadata,
            &TrashRestoreMetadata {
                name: "missing.md".to_string(),
                original_path: original,
                deleted_at_nanos: 10,
                can_restore: true,
                can_delete_permanently: true,
                permission_issue: None,
            },
        )
        .unwrap();

        let err =
            Operator::new(OperationContext::new(&journal).with_trash_metadata_path(&metadata))
                .execute(Operation::Delete {
                    path: trashed.clone(),
                })
                .unwrap_err();

        assert!(matches!(err, GfmError::Io { .. }));
        assert!(read_trash_metadata(&metadata)
            .unwrap()
            .contains_key("missing.md"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_trash_deletes_children_and_removes_metadata_after_success() {
        let root = unique_temp_dir("gfm-ops-empty-trash");
        let journal = root.join("journal.log");
        let metadata = root.join("trash.tsv");
        let trash_dir = root.join("Trash");
        let trashed_file = trash_dir.join("report.md");
        let trashed_dir = trash_dir.join("Old Folder");
        fs::create_dir_all(trashed_dir.join("nested")).unwrap();
        fs::write(&trashed_file, "delete file").unwrap();
        fs::write(trashed_dir.join("nested").join("note.txt"), "delete folder").unwrap();
        append_trash_metadata_entry(
            &metadata,
            &TrashRestoreMetadata {
                name: "report.md".to_string(),
                original_path: root.join("Documents").join("report.md"),
                deleted_at_nanos: 11,
                can_restore: true,
                can_delete_permanently: true,
                permission_issue: None,
            },
        )
        .unwrap();
        append_trash_metadata_entry(
            &metadata,
            &TrashRestoreMetadata {
                name: "Old Folder".to_string(),
                original_path: root.join("Documents").join("Old Folder"),
                deleted_at_nanos: 12,
                can_restore: true,
                can_delete_permanently: true,
                permission_issue: None,
            },
        )
        .unwrap();

        let mut progress = Vec::new();
        let entry =
            Operator::new(OperationContext::new(&journal).with_trash_metadata_path(&metadata))
                .execute_with_progress(
                    Operation::EmptyTrash {
                        path: trash_dir.clone(),
                    },
                    |event| progress.push(event),
                )
                .unwrap();

        assert_eq!(entry.status, OperationStatus::Completed);
        assert!(trash_dir.exists());
        assert!(fs::read_dir(&trash_dir).unwrap().next().is_none());
        assert!(read_trash_metadata(&metadata).unwrap().is_empty());
        assert_eq!(
            read_journal(&journal).unwrap().last().unwrap().operation,
            Operation::EmptyTrash { path: trash_dir }
        );
        assert_eq!(
            progress.last().unwrap().progress.completed_items,
            progress.last().unwrap().progress.total_items
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_trash_requires_directory_and_preserves_metadata() {
        let root = unique_temp_dir("gfm-ops-empty-trash-file");
        let journal = root.join("journal.log");
        let metadata = root.join("trash.tsv");
        let not_directory = root.join("Trash");
        fs::write(&not_directory, "not a directory").unwrap();
        append_trash_metadata_entry(
            &metadata,
            &TrashRestoreMetadata {
                name: "Trash".to_string(),
                original_path: root.join("Documents").join("Trash"),
                deleted_at_nanos: 13,
                can_restore: true,
                can_delete_permanently: true,
                permission_issue: None,
            },
        )
        .unwrap();

        let err =
            Operator::new(OperationContext::new(&journal).with_trash_metadata_path(&metadata))
                .execute(Operation::EmptyTrash {
                    path: not_directory.clone(),
                })
                .unwrap_err();

        assert!(
            matches!(err, GfmError::Format(message) if message.contains("requires a directory"))
        );
        assert!(not_directory.exists());
        assert!(read_trash_metadata(&metadata)
            .unwrap()
            .contains_key("Trash"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_trash_reconciles_stale_metadata_for_missing_children() {
        let root = unique_temp_dir("gfm-ops-empty-trash-stale-metadata");
        let journal = root.join("journal.log");
        let metadata = root.join("trash.tsv");
        let trash_dir = root.join("Trash");
        fs::create_dir_all(&trash_dir).unwrap();
        append_trash_metadata_entry(
            &metadata,
            &TrashRestoreMetadata {
                name: "already-deleted.md".to_string(),
                original_path: root.join("Documents").join("already-deleted.md"),
                deleted_at_nanos: 14,
                can_restore: true,
                can_delete_permanently: true,
                permission_issue: None,
            },
        )
        .unwrap();

        let entry =
            Operator::new(OperationContext::new(&journal).with_trash_metadata_path(&metadata))
                .execute(Operation::EmptyTrash {
                    path: trash_dir.clone(),
                })
                .unwrap();

        assert_eq!(entry.status, OperationStatus::Completed);
        assert!(trash_dir.exists());
        assert!(fs::read_dir(&trash_dir).unwrap().next().is_none());
        assert!(read_trash_metadata(&metadata).unwrap().is_empty());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn journals_failed_preflight_for_missing_source() {
        let root = unique_temp_dir("gfm-ops-missing-source");
        let journal = root.join("journal.log");
        let source = root.join("missing.txt");
        let destination = root.join("destination.txt");

        let operator = Operator::new(OperationContext::new(&journal));
        let err = operator
            .execute(Operation::Copy {
                from: source,
                to: destination,
            })
            .unwrap_err();

        assert!(matches!(err, GfmError::Io { .. }));
        let journal_entries = operator.journal().unwrap();
        assert_eq!(journal_entries.len(), 2);
        assert_eq!(journal_entries[0].status, OperationStatus::Started);
        assert_eq!(journal_entries[1].status, OperationStatus::Failed);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovers_interrupted_copy_with_original_operation_id() {
        let root = unique_temp_dir("gfm-ops-recover-copy");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "recover me").unwrap();
        append_journal(
            &journal,
            &JournalEntry::started(
                42,
                Operation::Copy {
                    from: source.clone(),
                    to: destination.clone(),
                },
            ),
        )
        .unwrap();

        let report = Operator::new(OperationContext::new(&journal))
            .recover_interrupted()
            .unwrap();

        assert_eq!(fs::read_to_string(&destination).unwrap(), "recover me");
        assert_eq!(report.outcomes.len(), 1);
        assert_eq!(report.outcomes[0].id, 42);
        assert_eq!(report.outcomes[0].status, OperationStatus::Completed);
        let entries = read_journal(&journal).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, 42);
        assert_eq!(entries[0].status, OperationStatus::Started);
        assert_eq!(entries[1].id, 42);
        assert_eq!(entries[1].status, OperationStatus::Completed);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_ignores_operations_with_terminal_status() {
        let root = unique_temp_dir("gfm-ops-recover-terminal");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "done").unwrap();
        append_journal(
            &journal,
            &JournalEntry::started(
                43,
                Operation::Copy {
                    from: source.clone(),
                    to: destination.clone(),
                },
            ),
        )
        .unwrap();
        append_journal(
            &journal,
            &JournalEntry::completed(
                43,
                Operation::Copy {
                    from: source.clone(),
                    to: destination.clone(),
                },
            ),
        )
        .unwrap();

        let report = Operator::new(OperationContext::new(&journal))
            .recover_interrupted()
            .unwrap();

        assert!(report.outcomes.is_empty());
        assert!(!destination.exists());
        assert_eq!(read_journal(&journal).unwrap().len(), 2);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_retries_classified_failed_operation_when_policy_allows_it() {
        let root = unique_temp_dir("gfm-ops-retry-failed");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        let operation = Operation::Copy {
            from: source.clone(),
            to: destination.clone(),
        };
        append_journal(&journal, &JournalEntry::started(44, operation.clone())).unwrap();
        append_journal(
            &journal,
            &JournalEntry::failed(
                44,
                operation.clone(),
                format!("{}: source does not exist", source.display()),
            ),
        )
        .unwrap();
        fs::write(&source, "arrived later").unwrap();

        let report = Operator::new(OperationContext::new(&journal))
            .recover_with_policy(OperationRecoveryPolicy {
                retry_failed: true,
                max_attempts: 2,
            })
            .unwrap();

        assert_eq!(report.outcomes.len(), 1);
        assert_eq!(report.outcomes[0].id, 44);
        assert_eq!(report.outcomes[0].status, OperationStatus::Completed);
        assert_eq!(fs::read_to_string(&destination).unwrap(), "arrived later");
        let entries = read_journal(&journal).unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[2].status, OperationStatus::Started);
        assert_eq!(entries[3].status, OperationStatus::Completed);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_does_not_retry_non_retryable_conflict_failures() {
        let root = unique_temp_dir("gfm-ops-retry-conflict");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "source").unwrap();
        fs::write(&destination, "destination").unwrap();
        let operation = Operation::Copy {
            from: source,
            to: destination.clone(),
        };
        append_journal(&journal, &JournalEntry::started(45, operation.clone())).unwrap();
        append_journal(
            &journal,
            &JournalEntry::failed(
                45,
                operation,
                format!("{}: destination already exists", destination.display()),
            ),
        )
        .unwrap();

        let report = Operator::new(OperationContext::new(&journal))
            .recover_with_policy(OperationRecoveryPolicy {
                retry_failed: true,
                max_attempts: 2,
            })
            .unwrap();

        assert!(report.outcomes.is_empty());
        assert_eq!(fs::read_to_string(&destination).unwrap(), "destination");
        assert_eq!(read_journal(&journal).unwrap().len(), 2);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn access_gate_prompts_before_mutating_destination_parent() {
        let root = unique_temp_dir("gfm-ops-access-prompt");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let protected = root.join("Documents");
        let destination = protected.join("destination.txt");
        fs::create_dir_all(&protected).unwrap();
        fs::write(&source, "source").unwrap();
        let gate = OperationAccessGate::new().with_decision(
            &protected,
            OperationAccessDecision::prompt("security-scoped bookmark required"),
        );

        let err = Operator::new(OperationContext::new(&journal).with_access_gate(gate))
            .execute(Operation::Copy {
                from: source,
                to: destination.clone(),
            })
            .unwrap_err();

        assert!(matches!(err, GfmError::Permission { .. }));
        assert!(!destination.exists());
        let entries = read_journal(&journal).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].status, OperationStatus::Started);
        assert_eq!(entries[1].status, OperationStatus::Failed);
        assert!(entries[1]
            .message
            .as_deref()
            .unwrap()
            .contains("permission prompt"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_does_not_retry_permission_prompt_failures() {
        let root = unique_temp_dir("gfm-ops-retry-permission");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "source").unwrap();
        let operation = Operation::Copy {
            from: source,
            to: destination.clone(),
        };
        append_journal(&journal, &JournalEntry::started(47, operation.clone())).unwrap();
        append_journal(
            &journal,
            &JournalEntry::failed(
                47,
                operation,
                "destination-parent requires a permission prompt before mutation".to_string(),
            ),
        )
        .unwrap();

        let report = Operator::new(OperationContext::new(&journal))
            .recover_with_policy(OperationRecoveryPolicy {
                retry_failed: true,
                max_attempts: 2,
            })
            .unwrap();

        assert!(report.outcomes.is_empty());
        assert!(!destination.exists());
        assert_eq!(read_journal(&journal).unwrap().len(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn moves_files_with_replace_policy() {
        let root = unique_temp_dir("gfm-ops-move");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "new").unwrap();
        fs::write(&destination, "old").unwrap();

        let operator =
            Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Replace));
        operator
            .execute(Operation::Move {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        assert!(!source.exists());
        assert_eq!(fs::read_to_string(&destination).unwrap(), "new");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelled_move_replace_regular_file_preserves_existing_destination() {
        let root = unique_temp_dir("gfm-ops-move-replace-cancel");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "new").unwrap();
        fs::write(&destination, "old").unwrap();
        let cancellation = OperationCancellation::default();
        let operator = Operator::new(
            OperationContext::new(&journal)
                .with_conflict(ConflictPolicy::Replace)
                .with_cancellation(cancellation.clone()),
        );

        let err = operator
            .execute_with_progress(
                Operation::Move {
                    from: source.clone(),
                    to: destination.clone(),
                },
                |event| {
                    if event.phase == OperationProgressPhase::Planned {
                        cancellation.cancel();
                    }
                },
            )
            .unwrap_err();

        assert!(matches!(err, GfmError::Cancelled));
        assert_eq!(fs::read_to_string(&source).unwrap(), "new");
        assert_eq!(fs::read_to_string(&destination).unwrap(), "old");

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn move_replace_same_inode_removes_source_link_without_rewriting_destination() {
        use std::os::unix::fs::MetadataExt;

        let root = unique_temp_dir("gfm-ops-move-replace-same-inode");
        let journal = root.join("journal.log");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "shared inode").unwrap();
        fs::hard_link(&source, &destination).unwrap();
        let before_destination = fs::metadata(&destination).unwrap();

        Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Replace))
            .execute(Operation::Move {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        let after_destination = fs::metadata(&destination).unwrap();
        assert!(!source.exists());
        assert_eq!(fs::read_to_string(&destination).unwrap(), "shared inode");
        assert_eq!(before_destination.ino(), after_destination.ino());
        assert_eq!(after_destination.nlink(), 1);

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn move_replace_symlink_uses_final_rename_without_predelete() {
        let root = unique_temp_dir("gfm-ops-move-replace-symlink");
        let journal = root.join("journal.log");
        let old_target = root.join("old-target.txt");
        let new_target = root.join("new-target.txt");
        let source = root.join("source-link");
        let destination = root.join("destination-link");
        fs::write(&old_target, "old").unwrap();
        fs::write(&new_target, "new").unwrap();
        std::os::unix::fs::symlink(&new_target, &source).unwrap();
        std::os::unix::fs::symlink(&old_target, &destination).unwrap();

        Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Replace))
            .execute(Operation::Move {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        assert!(!path_exists_or_symlink(&source));
        assert_eq!(fs::read_link(&destination).unwrap(), new_target);

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn cancelled_move_replace_symlink_preserves_existing_destination() {
        let root = unique_temp_dir("gfm-ops-move-replace-symlink-cancel");
        let journal = root.join("journal.log");
        let old_target = root.join("old-target.txt");
        let new_target = root.join("new-target.txt");
        let source = root.join("source-link");
        let destination = root.join("destination-link");
        fs::write(&old_target, "old").unwrap();
        fs::write(&new_target, "new").unwrap();
        std::os::unix::fs::symlink(&new_target, &source).unwrap();
        std::os::unix::fs::symlink(&old_target, &destination).unwrap();
        let cancellation = OperationCancellation::default();
        let operator = Operator::new(
            OperationContext::new(&journal)
                .with_conflict(ConflictPolicy::Replace)
                .with_cancellation(cancellation.clone()),
        );

        let err = operator
            .execute_with_progress(
                Operation::Move {
                    from: source.clone(),
                    to: destination.clone(),
                },
                |event| {
                    if event.phase == OperationProgressPhase::Planned {
                        cancellation.cancel();
                    }
                },
            )
            .unwrap_err();

        assert!(matches!(err, GfmError::Cancelled));
        assert_eq!(fs::read_link(&source).unwrap(), new_target);
        assert_eq!(fs::read_link(&destination).unwrap(), old_target);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn move_replace_directory_uses_final_rename_without_predelete() {
        let root = unique_temp_dir("gfm-ops-move-replace-directory");
        let journal = root.join("journal.log");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("nested").join("new.txt"), "new").unwrap();
        fs::create_dir_all(destination.join("nested")).unwrap();
        fs::write(destination.join("nested").join("old.txt"), "old").unwrap();

        Operator::new(OperationContext::new(&journal).with_conflict(ConflictPolicy::Replace))
            .execute(Operation::Move {
                from: source.clone(),
                to: destination.clone(),
            })
            .unwrap();

        assert!(!path_exists_or_symlink(&source));
        assert_eq!(
            fs::read_to_string(destination.join("nested").join("new.txt")).unwrap(),
            "new"
        );
        assert!(!destination.join("nested").join("old.txt").exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelled_move_replace_directory_preserves_existing_destination() {
        let root = unique_temp_dir("gfm-ops-move-replace-directory-cancel");
        let journal = root.join("journal.log");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("nested").join("new.txt"), "new").unwrap();
        fs::create_dir_all(destination.join("nested")).unwrap();
        fs::write(destination.join("nested").join("old.txt"), "old").unwrap();
        let cancellation = OperationCancellation::default();
        let operator = Operator::new(
            OperationContext::new(&journal)
                .with_conflict(ConflictPolicy::Replace)
                .with_cancellation(cancellation.clone()),
        );

        let err = operator
            .execute_with_progress(
                Operation::Move {
                    from: source.clone(),
                    to: destination.clone(),
                },
                |event| {
                    if event.phase == OperationProgressPhase::Planned {
                        cancellation.cancel();
                    }
                },
            )
            .unwrap_err();

        assert!(matches!(err, GfmError::Cancelled));
        assert_eq!(
            fs::read_to_string(source.join("nested").join("new.txt")).unwrap(),
            "new"
        );
        assert_eq!(
            fs::read_to_string(destination.join("nested").join("old.txt")).unwrap(),
            "old"
        );
        assert!(!destination.join("nested").join("new.txt").exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deletes_files_and_directories() {
        let root = unique_temp_dir("gfm-ops-delete");
        let journal = root.join("journal.log");
        let target = root.join("target");
        fs::create_dir_all(target.join("nested")).unwrap();
        fs::write(target.join("nested").join("file.txt"), "gone").unwrap();

        let operator = Operator::new(OperationContext::new(&journal));
        operator
            .execute(Operation::Delete {
                path: target.clone(),
            })
            .unwrap();

        assert!(!target.exists());
        fs::remove_dir_all(root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("{prefix}-{}", now_nanos()));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
