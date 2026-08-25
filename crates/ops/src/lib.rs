use gfm_types::{GfmError, Result};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    Fail,
    Replace,
    KeepBoth,
    Merge,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    Copy { from: PathBuf, to: PathBuf },
    Move { from: PathBuf, to: PathBuf },
    Rename { from: PathBuf, to: PathBuf },
    Delete { path: PathBuf },
    Trash { path: PathBuf },
    Restore { from: PathBuf, to: PathBuf },
}

impl Operation {
    pub fn target_path(&self) -> Option<&Path> {
        match self {
            Self::Copy { to, .. }
            | Self::Move { to, .. }
            | Self::Rename { to, .. }
            | Self::Restore { to, .. } => Some(to),
            Self::Delete { .. } | Self::Trash { .. } => None,
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
            Self::Delete { path } | Self::Trash { path } => vec![OperationAccessRequirement {
                path: path.clone(),
                role: OperationAccessRole::Target,
            }],
        }
    }
}

fn destination_probe_path(path: &Path) -> PathBuf {
    path.parent().unwrap_or(path).to_path_buf()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationAccessRole {
    Source,
    DestinationParent,
    Target,
}

impl OperationAccessRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::DestinationParent => "destination-parent",
            Self::Target => "target",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationAccessRequirement {
    pub path: PathBuf,
    pub role: OperationAccessRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationAccessAction {
    Allow,
    Prompt,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationAccessDecision {
    pub action: OperationAccessAction,
    pub reason: String,
}

impl OperationAccessDecision {
    pub fn allow(reason: impl Into<String>) -> Self {
        Self {
            action: OperationAccessAction::Allow,
            reason: reason.into(),
        }
    }

    pub fn prompt(reason: impl Into<String>) -> Self {
        Self {
            action: OperationAccessAction::Prompt,
            reason: reason.into(),
        }
    }

    pub fn deny(reason: impl Into<String>) -> Self {
        Self {
            action: OperationAccessAction::Deny,
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OperationAccessGate {
    decisions: BTreeMap<PathBuf, OperationAccessDecision>,
}

impl OperationAccessGate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_decision(
        mut self,
        path: impl Into<PathBuf>,
        decision: OperationAccessDecision,
    ) -> Self {
        self.decisions.insert(path.into(), decision);
        self
    }

    fn check(&self, operation: &Operation) -> Result<()> {
        for requirement in operation.access_requirements() {
            let Some(decision) = self.decisions.get(&requirement.path) else {
                continue;
            };
            match decision.action {
                OperationAccessAction::Allow => {}
                OperationAccessAction::Prompt => {
                    return Err(GfmError::Permission {
                        path: requirement.path,
                        message: format!(
                            "{} requires a permission prompt before mutation: {}",
                            requirement.role.as_str(),
                            decision.reason
                        ),
                    });
                }
                OperationAccessAction::Deny => {
                    return Err(GfmError::Permission {
                        path: requirement.path,
                        message: format!(
                            "{} is not accessible for mutation: {}",
                            requirement.role.as_str(),
                            decision.reason
                        ),
                    });
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationStatus {
    Started,
    Completed,
    Paused,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyMethod {
    ApfsClone,
    ByteCopy,
}

const COPY_BUFFER_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyExistingMode {
    Fresh,
    Resume,
    Merge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationPolicy {
    None,
    Size,
    Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationProgressPhase {
    Planned,
    Advanced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OperationProgress {
    pub total_items: u64,
    pub total_bytes: u64,
    pub completed_items: u64,
    pub completed_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationProgressEvent {
    pub phase: OperationProgressPhase,
    pub progress: OperationProgress,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    pub id: u128,
    pub status: OperationStatus,
    pub operation: Operation,
    pub message: Option<String>,
    pub timestamp_nanos: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationRecoveryReport {
    pub outcomes: Vec<OperationRecoveryOutcome>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationRecoveryPolicy {
    pub retry_failed: bool,
    pub max_attempts: usize,
}

impl Default for OperationRecoveryPolicy {
    fn default() -> Self {
        Self {
            retry_failed: false,
            max_attempts: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationRecoveryOutcome {
    pub id: u128,
    pub status: OperationStatus,
    pub operation: Operation,
    pub message: Option<String>,
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
}

impl OperationContext {
    pub fn new(journal_path: impl Into<PathBuf>) -> Self {
        Self {
            conflict: ConflictPolicy::Fail,
            journal_path: journal_path.into(),
            trash_metadata_path: None,
            cancellation: OperationCancellation::default(),
            pause: OperationPause::default(),
            verification: VerificationPolicy::Bytes,
            access_gate: OperationAccessGate::default(),
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
}

#[derive(Debug, Clone, Default)]
pub struct OperationCancellation(Arc<AtomicBool>);

impl OperationCancellation {
    pub fn cancel(&self) {
        self.0.store(true, AtomicOrdering::SeqCst);
    }

    pub fn check(&self) -> Result<()> {
        if self.0.load(AtomicOrdering::SeqCst) {
            Err(GfmError::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct OperationPause(Arc<AtomicBool>);

impl OperationPause {
    pub fn pause(&self) {
        self.0.store(true, AtomicOrdering::SeqCst);
    }

    pub fn check(&self) -> Result<()> {
        if self.0.load(AtomicOrdering::SeqCst) {
            Err(GfmError::Paused)
        } else {
            Ok(())
        }
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
                    status: if matches!(err, GfmError::Paused) {
                        OperationStatus::Paused
                    } else if matches!(err, GfmError::Cancelled) {
                        OperationStatus::Cancelled
                    } else {
                        OperationStatus::Failed
                    },
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
                resuming,
                progress,
            ),
            Operation::Move { from, to } | Operation::Rename { from, to } => move_path(
                from,
                to,
                self.context.conflict,
                self.context.verification,
                resuming,
                progress,
            ),
            Operation::Delete { path } => delete_path(path, progress),
            Operation::Trash { path } => {
                trash_path(path, self.context.trash_metadata_path.as_deref(), progress)
            }
            Operation::Restore { from, to } => restore_path(
                from,
                to,
                self.context.conflict,
                self.context.trash_metadata_path.as_deref(),
                progress,
            ),
        }
    }

    fn append(&self, entry: JournalEntry) -> Result<()> {
        append_journal(&self.context.journal_path, &entry)
    }
}

#[derive(Debug, Clone)]
struct OperationRecoveryState {
    id: u128,
    operation: Operation,
    last_status: OperationStatus,
    started_count: usize,
    message: Option<String>,
    timestamp_nanos: u128,
}

#[derive(Debug, Clone)]
struct OperationRecoveryPlan {
    entry: JournalEntry,
    append_started: bool,
}

fn recoverable_operations(
    entries: Vec<JournalEntry>,
    policy: OperationRecoveryPolicy,
) -> Vec<OperationRecoveryPlan> {
    let mut states: Vec<OperationRecoveryState> = Vec::new();
    for entry in entries {
        if let Some(state) = states.iter_mut().find(|state| state.id == entry.id) {
            let status = entry.status;
            state.operation = entry.operation;
            state.last_status = status;
            state.message = entry.message;
            if status == OperationStatus::Started {
                state.started_count += 1;
            }
            state.timestamp_nanos = entry.timestamp_nanos;
        } else {
            let started_count = usize::from(entry.status == OperationStatus::Started);
            states.push(OperationRecoveryState {
                id: entry.id,
                operation: entry.operation,
                last_status: entry.status,
                started_count,
                message: entry.message,
                timestamp_nanos: entry.timestamp_nanos,
            });
        }
    }
    states.sort_by_key(|state| (state.timestamp_nanos, state.id));
    states
        .into_iter()
        .filter_map(|state| {
            let failed_retryable = policy.retry_failed
                && state.last_status == OperationStatus::Failed
                && state.started_count < policy.max_attempts.max(1)
                && retryable_failure_message(state.message.as_deref());
            let append_started = if state.last_status == OperationStatus::Started {
                false
            } else if state.last_status == OperationStatus::Paused || failed_retryable {
                true
            } else {
                return None;
            };
            Some(OperationRecoveryPlan {
                entry: JournalEntry {
                    id: state.id,
                    status: state.last_status,
                    operation: state.operation,
                    message: state.message,
                    timestamp_nanos: state.timestamp_nanos,
                },
                append_started,
            })
        })
        .collect()
}

fn retryable_failure_message(message: Option<&str>) -> bool {
    let Some(message) = message else {
        return false;
    };
    let message = message.to_ascii_lowercase();
    [
        "source does not exist",
        "no such file",
        "resource temporarily unavailable",
        "operation timed out",
        "network is down",
        "network is unreachable",
        "device not configured",
        "stale file handle",
        "interrupted system call",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

impl JournalEntry {
    fn started(id: u128, operation: Operation) -> Self {
        Self {
            id,
            status: OperationStatus::Started,
            operation,
            message: None,
            timestamp_nanos: now_nanos(),
        }
    }

    fn completed(id: u128, operation: Operation) -> Self {
        Self {
            id,
            status: OperationStatus::Completed,
            operation,
            message: None,
            timestamp_nanos: now_nanos(),
        }
    }

    fn paused(id: u128, operation: Operation) -> Self {
        Self {
            id,
            status: OperationStatus::Paused,
            operation,
            message: None,
            timestamp_nanos: now_nanos(),
        }
    }

    fn cancelled(id: u128, operation: Operation) -> Self {
        Self {
            id,
            status: OperationStatus::Cancelled,
            operation,
            message: None,
            timestamp_nanos: now_nanos(),
        }
    }

    fn failed(id: u128, operation: Operation, message: String) -> Self {
        Self {
            id,
            status: OperationStatus::Failed,
            operation,
            message: Some(message),
            timestamp_nanos: now_nanos(),
        }
    }

    fn from_error(id: u128, operation: Operation, err: &GfmError) -> Self {
        if matches!(err, GfmError::Paused) {
            Self::paused(id, operation)
        } else if matches!(err, GfmError::Cancelled) {
            Self::cancelled(id, operation)
        } else {
            Self::failed(id, operation, err.to_string())
        }
    }
}

pub fn plan_operation(operation: &Operation) -> Result<OperationProgress> {
    plan_operation_checked(operation, &OperationCancellation::default())
}

fn plan_operation_checked(
    operation: &Operation,
    cancellation: &OperationCancellation,
) -> Result<OperationProgress> {
    match operation {
        Operation::Copy { from, .. }
        | Operation::Move { from, .. }
        | Operation::Rename { from, .. }
        | Operation::Restore { from, .. } => plan_path(from, cancellation),
        Operation::Delete { path } | Operation::Trash { path } => plan_path(path, cancellation),
    }
}

fn plan_path(path: &Path, cancellation: &OperationCancellation) -> Result<OperationProgress> {
    cancellation.check()?;
    let metadata = fs::symlink_metadata(path).map_err(|err| GfmError::io(path, err))?;
    let mut progress = OperationProgress {
        total_items: 1,
        total_bytes: item_bytes(&metadata),
        completed_items: 0,
        completed_bytes: 0,
    };
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(|err| GfmError::io(path, err))? {
            cancellation.check()?;
            let entry = entry.map_err(|err| GfmError::io(path, err))?;
            let child = plan_path(&entry.path(), cancellation)?;
            progress.total_items += child.total_items;
            progress.total_bytes += child.total_bytes;
        }
    }
    Ok(progress)
}

struct ProgressTracker<'a, F: FnMut(OperationProgressEvent)> {
    progress: OperationProgress,
    cancellation: &'a OperationCancellation,
    pause: &'a OperationPause,
    on_progress: &'a mut F,
}

impl<'a, F: FnMut(OperationProgressEvent)> ProgressTracker<'a, F> {
    fn new(
        plan: OperationProgress,
        cancellation: &'a OperationCancellation,
        pause: &'a OperationPause,
        on_progress: &'a mut F,
    ) -> Self {
        let mut tracker = Self {
            progress: plan,
            cancellation,
            pause,
            on_progress,
        };
        tracker.emit(OperationProgressPhase::Planned);
        tracker
    }

    fn advance(&mut self, metadata: &fs::Metadata) -> Result<()> {
        self.check_control()?;
        self.progress.completed_items += 1;
        self.progress.completed_bytes += item_bytes(metadata);
        self.emit(OperationProgressPhase::Advanced);
        self.check_control()
    }

    fn advance_bytes(&mut self, bytes: u64) -> Result<()> {
        self.check_control()?;
        self.progress.completed_bytes =
            (self.progress.completed_bytes + bytes).min(self.progress.total_bytes);
        self.emit(OperationProgressPhase::Advanced);
        self.check_control()
    }

    fn finish_current_item(&mut self) -> Result<()> {
        self.check_control()?;
        self.progress.completed_items += 1;
        self.emit(OperationProgressPhase::Advanced);
        self.check_control()
    }

    fn complete(&mut self) -> Result<()> {
        self.check_control()?;
        self.progress.completed_items = self.progress.total_items;
        self.progress.completed_bytes = self.progress.total_bytes;
        self.emit(OperationProgressPhase::Advanced);
        self.check_control()
    }

    fn emit(&mut self, phase: OperationProgressPhase) {
        (self.on_progress)(OperationProgressEvent {
            phase,
            progress: self.progress,
        });
    }

    fn check_cancelled(&self) -> Result<()> {
        self.check_control()
    }

    fn check_control(&self) -> Result<()> {
        self.cancellation.check().and_then(|()| self.pause.check())
    }
}

fn item_bytes(metadata: &fs::Metadata) -> u64 {
    if metadata.is_file() {
        metadata.len()
    } else {
        0
    }
}

fn copy_path(
    from: &Path,
    to: &Path,
    conflict: ConflictPolicy,
    verification: VerificationPolicy,
    resuming: bool,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    ensure_source_exists(from)?;
    let metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    if resuming && path_exists_or_symlink(to) {
        return copy_path_existing(
            from,
            to,
            &metadata,
            verification,
            CopyExistingMode::Resume,
            progress,
        );
    }
    if conflict == ConflictPolicy::Merge && metadata.is_dir() && path_exists_or_symlink(to) {
        return copy_path_existing(
            from,
            to,
            &metadata,
            verification,
            CopyExistingMode::Merge,
            progress,
        );
    }
    prepare_destination(to, conflict)?;
    if metadata.file_type().is_symlink() {
        copy_symlink(from, to, progress)
    } else if metadata.is_dir() {
        copy_directory(from, to, verification, CopyExistingMode::Fresh, progress)
    } else {
        copy_file_tracked(from, to, verification, progress)?;
        Ok(())
    }
}

fn move_path(
    from: &Path,
    to: &Path,
    conflict: ConflictPolicy,
    verification: VerificationPolicy,
    resuming: bool,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    ensure_source_exists(from)?;
    if resuming && path_exists_or_symlink(to) {
        let metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
        copy_path_existing(
            from,
            to,
            &metadata,
            verification,
            CopyExistingMode::Resume,
            progress,
        )?;
        delete_path_untracked(from)?;
        return Ok(());
    }
    if conflict == ConflictPolicy::Merge && path_exists_or_symlink(to) {
        let metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
        copy_path_existing(
            from,
            to,
            &metadata,
            verification,
            CopyExistingMode::Merge,
            progress,
        )?;
        delete_path_untracked(from)?;
        return Ok(());
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

fn delete_path(
    path: &Path,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    let metadata = fs::symlink_metadata(path).map_err(|err| GfmError::io(path, err))?;
    if metadata.is_dir() {
        fs::remove_dir_all(path).map_err(|err| GfmError::io(path, err))?;
        progress.complete()
    } else {
        fs::remove_file(path).map_err(|err| GfmError::io(path, err))?;
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

fn restore_path(
    from: &Path,
    to: &Path,
    conflict: ConflictPolicy,
    metadata_path: Option<&Path>,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    move_path(
        from,
        to,
        conflict,
        VerificationPolicy::Bytes,
        false,
        progress,
    )?;
    if let Some(metadata_path) = metadata_path {
        remove_trash_metadata(metadata_path, from)?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashRestoreMetadata {
    pub name: String,
    pub original_path: PathBuf,
    pub deleted_at_nanos: u128,
    pub can_restore: bool,
    pub can_delete_permanently: bool,
    pub permission_issue: Option<String>,
}

impl TrashRestoreMetadata {
    fn from_original_path(path: &Path) -> Result<Self> {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                GfmError::Format(format!(
                    "could not derive trash metadata name for {}",
                    path.display()
                ))
            })?;
        Ok(Self {
            name,
            original_path: path.to_path_buf(),
            deleted_at_nanos: now_nanos(),
            can_restore: true,
            can_delete_permanently: true,
            permission_issue: None,
        })
    }

    fn as_tsv(&self) -> String {
        [
            escape(&self.name),
            escape(&path_string(&self.original_path)),
            self.deleted_at_nanos.to_string(),
            self.can_restore.to_string(),
            self.can_delete_permanently.to_string(),
            escape(self.permission_issue.as_deref().unwrap_or("")),
        ]
        .join("\t")
    }
}

pub fn read_trash_metadata(
    path: impl AsRef<Path>,
) -> Result<BTreeMap<String, TrashRestoreMetadata>> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let file = File::open(path).map_err(|err| GfmError::io(path, err))?;
    let reader = BufReader::new(file);
    let mut entries = BTreeMap::new();
    for (line_index, line) in reader.lines().enumerate() {
        let line = line.map_err(|err| GfmError::io(path, err))?;
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 6 {
            return Err(GfmError::Format(format!(
                "{}:{} expected 6 tab-separated fields: name, original_path, deleted_at_nanos, can_restore, can_delete_permanently, permission_issue",
                path.display(),
                line_index + 1
            )));
        }
        let name = unescape(fields[0]).map_err(GfmError::Format)?;
        let original_path = PathBuf::from(unescape(fields[1]).map_err(GfmError::Format)?);
        let deleted_at_nanos = fields[2].parse().map_err(|err| {
            GfmError::Format(format!(
                "{}:{} invalid deleted_at_nanos `{}`: {err}",
                path.display(),
                line_index + 1,
                fields[2]
            ))
        })?;
        let can_restore = parse_bool_field(fields[3], "can_restore", path, line_index + 1)?;
        let can_delete_permanently =
            parse_bool_field(fields[4], "can_delete_permanently", path, line_index + 1)?;
        let permission_issue = unescape(fields[5])
            .map_err(GfmError::Format)
            .map(|value| (!value.is_empty()).then_some(value))?;
        entries.insert(
            name.clone(),
            TrashRestoreMetadata {
                name,
                original_path,
                deleted_at_nanos,
                can_restore,
                can_delete_permanently,
                permission_issue,
            },
        );
    }
    Ok(entries)
}

fn append_trash_metadata(path: &Path, original_path: &Path) -> Result<()> {
    append_trash_metadata_entry(
        path,
        &TrashRestoreMetadata::from_original_path(original_path)?,
    )
}

fn append_trash_metadata_entry(path: &Path, entry: &TrashRestoreMetadata) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    let mut entries = read_trash_metadata(path)?;
    entries.insert(entry.name.clone(), entry.clone());
    write_trash_metadata(path, entries.values())
}

fn remove_trash_metadata(path: &Path, trashed_path: &Path) -> Result<()> {
    let Some(name) = trashed_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
    else {
        return Ok(());
    };
    let mut entries = read_trash_metadata(path)?;
    entries.remove(&name);
    write_trash_metadata(path, entries.values())
}

fn write_trash_metadata<'a>(
    path: &Path,
    entries: impl IntoIterator<Item = &'a TrashRestoreMetadata>,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut file = File::create(&tmp).map_err(|err| GfmError::io(&tmp, err))?;
        for entry in entries {
            writeln!(file, "{}", entry.as_tsv()).map_err(|err| GfmError::io(&tmp, err))?;
        }
        file.flush().map_err(|err| GfmError::io(&tmp, err))?;
    }
    fs::rename(&tmp, path).map_err(|err| GfmError::io(path, err))
}

fn parse_bool_field(value: &str, name: &str, path: &Path, line: usize) -> Result<bool> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(GfmError::Format(format!(
            "{}:{} invalid {name} `{other}`",
            path.display(),
            line
        ))),
    }
}

fn copy_directory(
    from: &Path,
    to: &Path,
    verification: VerificationPolicy,
    mode: CopyExistingMode,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    let metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    if mode != CopyExistingMode::Fresh && path_exists_or_symlink(to) {
        let destination_metadata = fs::symlink_metadata(to).map_err(|err| GfmError::io(to, err))?;
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
    }
    preserve_metadata(from, to, &metadata)?;
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
                    verification,
                    CopyExistingMode::Resume,
                    progress,
                )?;
            } else if mode == CopyExistingMode::Merge && path_exists_or_symlink(&destination) {
                copy_path_existing(
                    &source,
                    &destination,
                    &child_metadata,
                    verification,
                    CopyExistingMode::Merge,
                    progress,
                )?;
            } else {
                copy_symlink(&source, &destination, progress)?;
            }
        } else if child_metadata.is_dir() {
            copy_directory(&source, &destination, verification, mode, progress)?;
        } else if mode == CopyExistingMode::Resume && path_exists_or_symlink(&destination) {
            copy_path_existing(
                &source,
                &destination,
                &child_metadata,
                verification,
                CopyExistingMode::Resume,
                progress,
            )?;
        } else if mode == CopyExistingMode::Merge && path_exists_or_symlink(&destination) {
            copy_path_existing(
                &source,
                &destination,
                &child_metadata,
                verification,
                CopyExistingMode::Merge,
                progress,
            )?;
        } else {
            let _ = copy_file_tracked(&source, &destination, verification, progress)?;
        }
    }
    Ok(())
}

fn copy_path_existing(
    from: &Path,
    to: &Path,
    metadata: &fs::Metadata,
    verification: VerificationPolicy,
    mode: CopyExistingMode,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    if metadata.file_type().is_symlink() {
        verify_existing_symlink_copy(from, to, mode)?;
        progress.advance(metadata)
    } else if metadata.is_dir() {
        copy_directory(from, to, verification, mode, progress)
    } else if mode == CopyExistingMode::Merge {
        Err(GfmError::Conflict {
            path: to.to_path_buf(),
            message: "merge destination file already exists".to_string(),
        })
    } else {
        verify_copy(from, to, verification)?;
        preserve_metadata(from, to, metadata)?;
        progress.advance(metadata)
    }
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
            copy_file_bytes_tracked(from, to, progress)?;
            preserve_metadata(from, to, &metadata)?;
            verify_copy(from, to, verification)?;
            progress.finish_current_item()?;
            Ok(CopyMethod::ByteCopy)
        }
        Err(err) => Err(GfmError::io(from, err)),
    }
}

#[cfg(test)]
fn copy_file_bytes(from: &Path, to: &Path) -> Result<u64> {
    let mut source = File::open(from).map_err(|err| GfmError::io(from, err))?;
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
        if let Err(err) = destination.write_all(&buffer[..read]) {
            break Err(GfmError::io(to, err));
        }
        written += read as u64;
    };

    if result.is_err() {
        let _ = fs::remove_file(to);
    }
    result
}

fn copy_file_bytes_tracked(
    from: &Path,
    to: &Path,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<u64> {
    let mut source = File::open(from).map_err(|err| GfmError::io(from, err))?;
    let mut destination = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)
        .map_err(|err| GfmError::io(to, err))?;
    let mut buffer = vec![0; COPY_BUFFER_BYTES];
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
        if let Err(err) = destination.write_all(&buffer[..read]) {
            break Err(GfmError::io(to, err));
        }
        written += read as u64;
        if let Err(err) = progress.advance_bytes(read as u64) {
            break Err(err);
        }
    };

    if result.is_err() {
        let _ = fs::remove_file(to);
    }
    result
}

fn verify_copy(from: &Path, to: &Path, policy: VerificationPolicy) -> Result<()> {
    match policy {
        VerificationPolicy::None => Ok(()),
        VerificationPolicy::Size => verify_copy_size(from, to),
        VerificationPolicy::Bytes => {
            verify_copy_size(from, to)?;
            verify_copy_bytes(from, to)
        }
    }
}

fn verify_copy_size(from: &Path, to: &Path) -> Result<()> {
    let source_len = fs::metadata(from)
        .map_err(|err| GfmError::io(from, err))?
        .len();
    let destination_len = fs::metadata(to).map_err(|err| GfmError::io(to, err))?.len();
    if source_len == destination_len {
        Ok(())
    } else {
        Err(GfmError::Format(format!(
            "copy verification failed for {} -> {}: source size {} != destination size {}",
            from.display(),
            to.display(),
            source_len,
            destination_len
        )))
    }
}

fn verify_copy_bytes(from: &Path, to: &Path) -> Result<()> {
    const VERIFY_BUFFER_BYTES: usize = 128 * 1024;

    let mut source = File::open(from).map_err(|err| GfmError::io(from, err))?;
    let mut destination = File::open(to).map_err(|err| GfmError::io(to, err))?;
    let mut source_buffer = vec![0; VERIFY_BUFFER_BYTES];
    let mut destination_buffer = vec![0; VERIFY_BUFFER_BYTES];
    let mut offset = 0_u64;

    loop {
        let source_read = source
            .read(&mut source_buffer)
            .map_err(|err| GfmError::io(from, err))?;
        let destination_read = destination
            .read(&mut destination_buffer)
            .map_err(|err| GfmError::io(to, err))?;
        if source_read != destination_read {
            return Err(GfmError::Format(format!(
                "copy verification failed for {} -> {}: read length drift at byte {}",
                from.display(),
                to.display(),
                offset
            )));
        }
        if source_read == 0 {
            return Ok(());
        }
        if source_buffer[..source_read] != destination_buffer[..destination_read] {
            return Err(GfmError::Format(format!(
                "copy verification failed for {} -> {}: byte mismatch in block starting at {}",
                from.display(),
                to.display(),
                offset
            )));
        }
        offset += source_read as u64;
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

fn preserve_metadata(from: &Path, to: &Path, metadata: &fs::Metadata) -> Result<()> {
    preserve_ownership(to, metadata)?;
    preserve_permissions(to, metadata)?;
    preserve_times(to, metadata)?;
    preserve_xattrs(from, to)
}

#[cfg(unix)]
fn preserve_ownership(to: &Path, metadata: &fs::Metadata) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    match rustix::fs::chown(
        to,
        Some(rustix::fs::Uid::from_raw(metadata.uid())),
        Some(rustix::fs::Gid::from_raw(metadata.gid())),
    ) {
        Ok(()) => Ok(()),
        Err(err) => {
            let err = io::Error::from(err);
            if ownership_preservation_unsupported(&err) {
                Ok(())
            } else {
                Err(GfmError::io(to, err))
            }
        }
    }
}

#[cfg(unix)]
fn ownership_preservation_unsupported(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(libc::EPERM) | Some(libc::EACCES) | Some(libc::ENOTSUP)
    ) || matches!(
        err.kind(),
        io::ErrorKind::PermissionDenied | io::ErrorKind::Unsupported
    )
}

#[cfg(not(unix))]
fn preserve_ownership(_to: &Path, _metadata: &fs::Metadata) -> Result<()> {
    Ok(())
}

fn preserve_permissions(to: &Path, metadata: &fs::Metadata) -> Result<()> {
    fs::set_permissions(to, metadata.permissions()).map_err(|err| GfmError::io(to, err))
}

fn preserve_times(to: &Path, metadata: &fs::Metadata) -> Result<()> {
    let atime = filetime::FileTime::from_last_access_time(metadata);
    let mtime = filetime::FileTime::from_last_modification_time(metadata);
    filetime::set_file_times(to, atime, mtime).map_err(|err| GfmError::io(to, err))
}

fn preserve_xattrs(from: &Path, to: &Path) -> Result<()> {
    let names = match xattr::list(from) {
        Ok(names) => names,
        Err(err) if xattr_copy_unsupported(&err) => return Ok(()),
        Err(err) => return Err(GfmError::io(from, err)),
    };
    for name in names {
        let value = match xattr::get(from, &name) {
            Ok(Some(value)) => value,
            Ok(None) => continue,
            Err(err) if xattr_copy_unsupported(&err) => continue,
            Err(err) => return Err(GfmError::io(from, err)),
        };
        match xattr::set(to, &name, &value) {
            Ok(()) => {}
            Err(err) if xattr_copy_unsupported(&err) => {}
            Err(err) => return Err(GfmError::io(to, err)),
        }
    }
    Ok(())
}

fn xattr_copy_unsupported(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(libc::ENOTSUP)
            | Some(libc::ENODATA)
            | Some(libc::ENOATTR)
            | Some(libc::EPERM)
            | Some(libc::EACCES)
    ) || matches!(
        err.kind(),
        io::ErrorKind::Unsupported | io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
    )
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
        Operation::Restore { from, to } => Ok(Operation::Restore {
            from,
            to: keep_both_path(&to)?,
        }),
    }
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
    }
}

fn append_journal(path: &Path, entry: &JournalEntry) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|err| GfmError::io(path, err))?;
    writeln!(file, "{}", encode_entry(entry)).map_err(|err| GfmError::io(path, err))?;
    file.flush().map_err(|err| GfmError::io(path, err))
}

pub fn read_journal(path: impl AsRef<Path>) -> Result<Vec<JournalEntry>> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = File::open(path).map_err(|err| GfmError::io(path, err))?;
    let reader = BufReader::new(file);
    let mut entries = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        let line = line.map_err(|err| GfmError::io(path, err))?;
        entries.push(parse_entry(&line).map_err(|err| {
            GfmError::Format(format!("{} line {}: {}", path.display(), index + 1, err))
        })?);
    }
    Ok(entries)
}

fn encode_entry(entry: &JournalEntry) -> String {
    let (op, from, to) = encode_operation(&entry.operation);
    [
        entry.id.to_string(),
        encode_status(entry.status).to_string(),
        entry.timestamp_nanos.to_string(),
        op.to_string(),
        escape(&from),
        escape(&to),
        escape(entry.message.as_deref().unwrap_or("")),
    ]
    .join("\t")
}

fn parse_entry(line: &str) -> std::result::Result<JournalEntry, String> {
    let parts: Vec<_> = line.split('\t').collect();
    if parts.len() != 7 {
        return Err(format!("expected 7 fields, got {}", parts.len()));
    }
    let id = parts[0]
        .parse()
        .map_err(|err| format!("invalid operation id `{}`: {err}", parts[0]))?;
    let status = decode_status(parts[1])?;
    let timestamp_nanos = parts[2]
        .parse()
        .map_err(|err| format!("invalid timestamp `{}`: {err}", parts[2]))?;
    let operation = decode_operation(parts[3], &unescape(parts[4])?, &unescape(parts[5])?)?;
    let message = unescape(parts[6])?;
    Ok(JournalEntry {
        id,
        status,
        operation,
        message: (!message.is_empty()).then_some(message),
        timestamp_nanos,
    })
}

fn encode_operation(operation: &Operation) -> (&'static str, String, String) {
    match operation {
        Operation::Copy { from, to } => ("copy", path_string(from), path_string(to)),
        Operation::Move { from, to } => ("move", path_string(from), path_string(to)),
        Operation::Rename { from, to } => ("rename", path_string(from), path_string(to)),
        Operation::Delete { path } => ("delete", path_string(path), String::new()),
        Operation::Trash { path } => ("trash", path_string(path), String::new()),
        Operation::Restore { from, to } => ("restore", path_string(from), path_string(to)),
    }
}

fn decode_operation(kind: &str, from: &str, to: &str) -> std::result::Result<Operation, String> {
    match kind {
        "copy" => Ok(Operation::Copy {
            from: PathBuf::from(from),
            to: PathBuf::from(to),
        }),
        "move" => Ok(Operation::Move {
            from: PathBuf::from(from),
            to: PathBuf::from(to),
        }),
        "rename" => Ok(Operation::Rename {
            from: PathBuf::from(from),
            to: PathBuf::from(to),
        }),
        "delete" => Ok(Operation::Delete {
            path: PathBuf::from(from),
        }),
        "trash" => Ok(Operation::Trash {
            path: PathBuf::from(from),
        }),
        "restore" => Ok(Operation::Restore {
            from: PathBuf::from(from),
            to: PathBuf::from(to),
        }),
        other => Err(format!("unknown operation `{other}`")),
    }
}

fn encode_status(status: OperationStatus) -> &'static str {
    match status {
        OperationStatus::Started => "started",
        OperationStatus::Completed => "completed",
        OperationStatus::Paused => "paused",
        OperationStatus::Cancelled => "cancelled",
        OperationStatus::Failed => "failed",
    }
}

fn decode_status(value: &str) -> std::result::Result<OperationStatus, String> {
    match value {
        "started" => Ok(OperationStatus::Started),
        "completed" => Ok(OperationStatus::Completed),
        "paused" => Ok(OperationStatus::Paused),
        "cancelled" => Ok(OperationStatus::Cancelled),
        "failed" => Ok(OperationStatus::Failed),
        other => Err(format!("unknown status `{other}`")),
    }
}

fn escape(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' => output.push_str("\\\\"),
            '\t' => output.push_str("\\t"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            other => output.push(other),
        }
    }
    output
}

fn unescape(input: &str) -> std::result::Result<String, String> {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        match chars.next() {
            Some('\\') => output.push('\\'),
            Some('t') => output.push('\t'),
            Some('n') => output.push('\n'),
            Some('r') => output.push('\r'),
            Some(other) => return Err(format!("invalid escape `\\{other}`")),
            None => return Err("trailing escape".to_string()),
        }
    }
    Ok(output)
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
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
        assert!(destination.is_dir());
        assert!(!destination.join("first.txt").exists());
        assert!(!destination.join("nested").exists());
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

        let copied = copy_file_bytes_tracked(&source, &destination, &mut tracker).unwrap();
        tracker.finish_current_item().unwrap();

        assert_eq!(copied, bytes.len() as u64);
        assert!(events
            .iter()
            .any(|event| event.phase == OperationProgressPhase::Advanced
                && event.progress.completed_items == 0
                && event.progress.completed_bytes == COPY_BUFFER_BYTES as u64));
        let last = events.last().unwrap();
        assert_eq!(last.progress.completed_items, 1);
        assert_eq!(last.progress.completed_bytes, bytes.len() as u64);
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

        let err = copy_file_bytes_tracked(&source, &destination, &mut tracker).unwrap_err();

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
