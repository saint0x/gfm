use gfm_types::{GfmError, Result};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    Fail,
    Replace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    Copy { from: PathBuf, to: PathBuf },
    Move { from: PathBuf, to: PathBuf },
    Rename { from: PathBuf, to: PathBuf },
    Delete { path: PathBuf },
    Trash { path: PathBuf },
}

impl Operation {
    pub fn target_path(&self) -> Option<&Path> {
        match self {
            Self::Copy { to, .. } | Self::Move { to, .. } | Self::Rename { to, .. } => Some(to),
            Self::Delete { .. } | Self::Trash { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationStatus {
    Started,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyMethod {
    ApfsClone,
    ByteCopy,
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
    pub cancellation: OperationCancellation,
}

impl OperationContext {
    pub fn new(journal_path: impl Into<PathBuf>) -> Self {
        Self {
            conflict: ConflictPolicy::Fail,
            journal_path: journal_path.into(),
            cancellation: OperationCancellation::default(),
        }
    }

    pub fn with_conflict(mut self, conflict: ConflictPolicy) -> Self {
        self.conflict = conflict;
        self
    }

    pub fn with_cancellation(mut self, cancellation: OperationCancellation) -> Self {
        self.cancellation = cancellation;
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
            match self.execute_started(entry.id, operation.clone(), &mut |_| {}) {
                Ok(completed) => outcomes.push(OperationRecoveryOutcome {
                    id: completed.id,
                    status: completed.status,
                    operation: completed.operation,
                    message: completed.message,
                }),
                Err(err) => outcomes.push(OperationRecoveryOutcome {
                    id: entry.id,
                    status: if matches!(err, GfmError::Cancelled) {
                        OperationStatus::Cancelled
                    } else {
                        OperationStatus::Failed
                    },
                    operation,
                    message: (!matches!(err, GfmError::Cancelled)).then(|| err.to_string()),
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
        let plan = match plan_operation_checked(&operation, &self.context.cancellation) {
            Ok(plan) => plan,
            Err(err) => {
                let entry = JournalEntry::from_error(id, operation, &err);
                let _ = self.append(entry);
                return Err(err);
            }
        };
        let mut progress = ProgressTracker::new(plan, &self.context.cancellation, on_progress);
        match self.apply(&operation, &mut progress) {
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
        progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
    ) -> Result<()> {
        match operation {
            Operation::Copy { from, to } => copy_path(from, to, self.context.conflict, progress),
            Operation::Move { from, to } | Operation::Rename { from, to } => {
                move_path(from, to, self.context.conflict, progress)
            }
            Operation::Delete { path } => delete_path(path, progress),
            Operation::Trash { path } => trash_path(path, progress),
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
            let append_started = if state.last_status == OperationStatus::Started {
                false
            } else if policy.retry_failed
                && state.last_status == OperationStatus::Failed
                && state.started_count < policy.max_attempts.max(1)
                && retryable_failure_message(state.message.as_deref())
            {
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
        if matches!(err, GfmError::Cancelled) {
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
        | Operation::Rename { from, .. } => plan_path(from, cancellation),
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
    on_progress: &'a mut F,
}

impl<'a, F: FnMut(OperationProgressEvent)> ProgressTracker<'a, F> {
    fn new(
        plan: OperationProgress,
        cancellation: &'a OperationCancellation,
        on_progress: &'a mut F,
    ) -> Self {
        let mut tracker = Self {
            progress: plan,
            cancellation,
            on_progress,
        };
        tracker.emit(OperationProgressPhase::Planned);
        tracker
    }

    fn advance(&mut self, metadata: &fs::Metadata) -> Result<()> {
        self.cancellation.check()?;
        self.progress.completed_items += 1;
        self.progress.completed_bytes += item_bytes(metadata);
        self.emit(OperationProgressPhase::Advanced);
        self.cancellation.check()
    }

    fn complete(&mut self) -> Result<()> {
        self.cancellation.check()?;
        self.progress.completed_items = self.progress.total_items;
        self.progress.completed_bytes = self.progress.total_bytes;
        self.emit(OperationProgressPhase::Advanced);
        self.cancellation.check()
    }

    fn emit(&mut self, phase: OperationProgressPhase) {
        (self.on_progress)(OperationProgressEvent {
            phase,
            progress: self.progress,
        });
    }

    fn check_cancelled(&self) -> Result<()> {
        self.cancellation.check()
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
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    ensure_source_exists(from)?;
    prepare_destination(to, conflict)?;
    let metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    if metadata.file_type().is_symlink() {
        copy_symlink(from, to, progress)
    } else if metadata.is_dir() {
        copy_directory(from, to, progress)
    } else {
        copy_file(from, to)?;
        progress.advance(&metadata)
    }
}

fn move_path(
    from: &Path,
    to: &Path,
    conflict: ConflictPolicy,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    ensure_source_exists(from)?;
    prepare_destination(to, conflict)?;
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    match fs::rename(from, to) {
        Ok(()) => progress.complete(),
        Err(rename_err) => {
            copy_path(from, to, ConflictPolicy::Replace, progress)?;
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
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    ensure_source_exists(path)?;
    trash::delete(path).map_err(|err| GfmError::io(path, err))?;
    progress.complete()
}

fn copy_directory(
    from: &Path,
    to: &Path,
    progress: &mut ProgressTracker<'_, impl FnMut(OperationProgressEvent)>,
) -> Result<()> {
    progress.check_cancelled()?;
    fs::create_dir_all(to).map_err(|err| GfmError::io(to, err))?;
    let metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
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
            copy_symlink(&source, &destination, progress)?;
        } else if child_metadata.is_dir() {
            copy_directory(&source, &destination, progress)?;
        } else {
            let _ = copy_file(&source, &destination)?;
            progress.advance(&child_metadata)?;
        }
    }
    Ok(())
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
    preserve_xattrs(from, to)?;
    let metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    progress.advance(&metadata)
}

fn copy_file(from: &Path, to: &Path) -> Result<CopyMethod> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    let metadata = fs::symlink_metadata(from).map_err(|err| GfmError::io(from, err))?;
    match clone_file(from, to) {
        Ok(()) => {
            preserve_metadata(from, to, &metadata)?;
            Ok(CopyMethod::ApfsClone)
        }
        Err(err) if clone_fallback_allowed(&err) => {
            remove_failed_clone_destination(to)?;
            fs::copy(from, to).map_err(|err| GfmError::io(from, err))?;
            preserve_metadata(from, to, &metadata)?;
            Ok(CopyMethod::ByteCopy)
        }
        Err(err) => Err(GfmError::io(from, err)),
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
    preserve_permissions(to, metadata)?;
    preserve_times(to, metadata)?;
    preserve_xattrs(from, to)
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

fn prepare_destination(path: &Path, conflict: ConflictPolicy) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    match conflict {
        ConflictPolicy::Fail => Err(GfmError::Conflict {
            path: path.to_path_buf(),
            message: "destination already exists".to_string(),
        }),
        ConflictPolicy::Replace => delete_path_untracked(path),
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
        other => Err(format!("unknown operation `{other}`")),
    }
}

fn encode_status(status: OperationStatus) -> &'static str {
    match status {
        OperationStatus::Started => "started",
        OperationStatus::Completed => "completed",
        OperationStatus::Cancelled => "cancelled",
        OperationStatus::Failed => "failed",
    }
}

fn decode_status(value: &str) -> std::result::Result<OperationStatus, String> {
    match value {
        "started" => Ok(OperationStatus::Started),
        "completed" => Ok(OperationStatus::Completed),
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
    fn copy_file_reports_method_and_preserves_contents() {
        let root = unique_temp_dir("gfm-ops-copy-method");
        let source = root.join("source.txt");
        let destination = root.join("destination.txt");
        fs::write(&source, "clone-aware copy").unwrap();

        let method = copy_file(&source, &destination).unwrap();

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
                let method = copy_file(&source, &destination).unwrap();
                assert_eq!(method, CopyMethod::ApfsClone);
                assert_eq!(fs::read(&destination).unwrap(), b"copy-on-write candidate");
            }
            Err(err) if clone_fallback_allowed(&err) => {
                let method = copy_file(&source, &destination).unwrap();
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
