use crate::access::{preflight_access_scope, ScopedAccessGuard};
use gfm_jobs::{
    Cancellation, Job, JobFairnessPolicy, JobJournal, JobPayloadCatalog, JobPayloadKind,
    JobPayloadRecord, JobProgressSnapshot, JobProgressState, JobProgressStore, Priority,
    RetriableTask, RetryPolicy, Scheduler, SchedulingAction, SchedulingPressure, Task, TaskStatus,
    VolumeConcurrencyPolicy, WorkerPool,
};
use gfm_mac::AccessIntent;
use gfm_ops::OperationConflictReport;
use gfm_types::{GfmError, Result, VolumeId};
use std::collections::BTreeSet;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn run_volume_task<T>(
    volume: Option<VolumeId>,
    priority: Priority,
    label: &'static str,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T>
where
    T: Send + 'static,
{
    run_volume_task_cancellable(volume, priority, label, move |_| work())
}

pub(crate) fn run_volume_task_cancellable<T>(
    volume: Option<VolumeId>,
    priority: Priority,
    label: &'static str,
    work: impl FnOnce(Cancellation) -> Result<T> + Send + 'static,
) -> Result<T>
where
    T: Send + 'static,
{
    let result_slot = Arc::new(Mutex::new(None));
    let result_slot_task = Arc::clone(&result_slot);
    let mut scheduler = Scheduler::new();
    let job = if let Some(volume) = volume {
        scheduler.schedule_on_volume(priority, label, volume)
    } else {
        scheduler.schedule(priority, label)
    };
    let job = drain_single_runtime_job(&mut scheduler, job, label)?;
    let runtime = RuntimeJobHandle::begin(
        &job,
        payload_kind_for_label(label),
        label,
        1,
        format!("{}:{label}", priority.as_str()),
    )?;
    let runtime_task = runtime.clone();
    let task = Task::new(job.clone(), move |cancellation| {
        runtime_task.running()?;
        let result = work(cancellation)?;
        *result_slot_task
            .lock()
            .expect("volume task result lock poisoned") = Some(result);
        Ok(())
    });
    let report = WorkerPool::new(1).run_isolated(vec![task], VolumeConcurrencyPolicy::new(1));
    let outcome = report
        .outcomes
        .iter()
        .find(|outcome| outcome.id == job.id)
        .ok_or_else(|| GfmError::Format(format!("{label} job did not run")))?;
    runtime.finish(&outcome.status)?;
    match &outcome.status {
        TaskStatus::Completed => {}
        TaskStatus::Started => {
            return Err(GfmError::Format(format!("{label} job is still running")))
        }
        TaskStatus::Cancelled => return Err(GfmError::Cancelled),
        TaskStatus::Failed(message) => {
            return Err(GfmError::Format(format!("{label} job failed: {message}")))
        }
    }
    let result = result_slot
        .lock()
        .expect("volume task result lock poisoned")
        .take()
        .ok_or_else(|| GfmError::Format(format!("{label} job completed without a result")))?;
    Ok(result)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScheduledTaskOutcome<T> {
    pub(crate) result: Option<T>,
    pub(crate) scheduling_action: SchedulingAction,
    pub(crate) deferred: bool,
}

pub(crate) fn run_scheduled_volume_task<T>(
    volume: Option<VolumeId>,
    priority: Priority,
    label: &'static str,
    pressure: SchedulingPressure,
    work: impl Fn() -> Result<T> + Send + Sync + 'static,
) -> Result<ScheduledTaskOutcome<T>>
where
    T: Send + 'static,
{
    run_scheduled_volume_task_cancellable(volume, priority, label, pressure, move |_| work())
}

pub(crate) fn run_scheduled_volume_task_cancellable<T>(
    volume: Option<VolumeId>,
    priority: Priority,
    label: &'static str,
    pressure: SchedulingPressure,
    work: impl Fn(Cancellation) -> Result<T> + Send + Sync + 'static,
) -> Result<ScheduledTaskOutcome<T>>
where
    T: Send + 'static,
{
    run_scheduled_volume_task_cancellable_with_volume(
        priority,
        label,
        pressure,
        || Ok(volume),
        work,
    )
}

pub(crate) fn run_scheduled_volume_task_cancellable_with_volume<T>(
    priority: Priority,
    label: &'static str,
    pressure: SchedulingPressure,
    volume: impl FnOnce() -> Result<Option<VolumeId>>,
    work: impl Fn(Cancellation) -> Result<T> + Send + Sync + 'static,
) -> Result<ScheduledTaskOutcome<T>>
where
    T: Send + 'static,
{
    let scheduling = pressure.decide(priority, 1, 1);
    let mut scheduler = Scheduler::new();
    let mut job = scheduler.schedule(priority, label);
    let journal = JobJournal::new(default_job_journal_path());
    if scheduling.action == SchedulingAction::Defer {
        let runtime = RuntimeJobHandle::begin(
            &job,
            payload_kind_for_label(label),
            label,
            1,
            format!("{}:{label}:adaptive", priority.as_str()),
        )?;
        runtime.deferred(scheduling.action)?;
        return Ok(ScheduledTaskOutcome {
            result: None,
            scheduling_action: scheduling.action,
            deferred: true,
        });
    }

    if let Some(volume) = volume()? {
        job = scheduler
            .bind_volume(job.id, volume)
            .ok_or_else(|| GfmError::Format(format!("{label} job was not queued")))?;
    }
    let _journal_access = (scheduling.action != SchedulingAction::Defer)
        .then(|| preflight_runtime_write(journal.path(), label))
        .transpose()?;
    let runtime = RuntimeJobHandle::begin(
        &job,
        payload_kind_for_label(label),
        label,
        1,
        format!("{}:{label}:adaptive", priority.as_str()),
    )?;

    let result_slot = Arc::new(Mutex::new(None));
    let result_slot_task = Arc::clone(&result_slot);
    let job = drain_single_runtime_job(&mut scheduler, job, label)?;
    let runtime_task = runtime.clone();
    let task = RetriableTask::new(job.clone(), move |cancellation| {
        runtime_task.running()?;
        let result = work(cancellation)?;
        *result_slot_task
            .lock()
            .expect("scheduled task result lock poisoned") = Some(result);
        Ok(())
    });
    let report = WorkerPool::new(scheduling.worker_threads).run_retriable_isolated(
        vec![task],
        &journal,
        RetryPolicy { max_attempts: 2 },
        scheduling.volume_policy,
    );
    let outcome = report
        .outcomes
        .iter()
        .find(|outcome| outcome.id == job.id)
        .ok_or_else(|| GfmError::Format(format!("{label} job did not run")))?;
    runtime.finish(&outcome.status)?;
    match &outcome.status {
        TaskStatus::Completed => {}
        TaskStatus::Started => {
            return Err(GfmError::Format(format!("{label} job is still running")))
        }
        TaskStatus::Cancelled => return Err(GfmError::Cancelled),
        TaskStatus::Failed(message) => {
            return Err(GfmError::Format(format!("{label} job failed: {message}")))
        }
    }
    let result = result_slot
        .lock()
        .expect("scheduled task result lock poisoned")
        .take()
        .ok_or_else(|| GfmError::Format(format!("{label} job completed without a result")))?;
    Ok(ScheduledTaskOutcome {
        result: Some(result),
        scheduling_action: scheduling.action,
        deferred: false,
    })
}

fn drain_single_runtime_job(scheduler: &mut Scheduler, job: Job, label: &str) -> Result<Job> {
    let plan = scheduler.drain_fair_ready(JobFairnessPolicy::default(), []);
    if let Some(blocked) = plan.blocked.first() {
        return Err(GfmError::Format(format!(
            "{label} job {} is blocked by missing dependencies",
            blocked.label
        )));
    }
    plan.ready
        .into_iter()
        .find(|candidate| candidate.id == job.id)
        .ok_or_else(|| GfmError::Format(format!("{label} job did not become ready")))
}

#[derive(Clone)]
pub(crate) struct RuntimeJobHandle {
    progress_store: Option<JobProgressStore>,
    last_progress: Arc<Mutex<JobProgressSnapshot>>,
}

impl RuntimeJobHandle {
    pub(crate) fn begin(
        job: &Job,
        kind: JobPayloadKind,
        label: &str,
        total_units: u64,
        summary: String,
    ) -> Result<Self> {
        Self::begin_with_payload_path(
            job,
            kind,
            label,
            runtime_payload_path(kind, label),
            total_units,
            summary,
        )
    }

    pub(crate) fn begin_with_payload_path(
        job: &Job,
        kind: JobPayloadKind,
        label: &str,
        payload_path: impl Into<PathBuf>,
        total_units: u64,
        summary: String,
    ) -> Result<Self> {
        let payload_path = payload_path.into();
        if let Some(catalog) = runtime_payload_catalog() {
            let _access = preflight_runtime_write(catalog.path(), label)?;
            catalog.append(&JobPayloadRecord::new(
                job.id,
                kind,
                label,
                payload_path,
                job.volume,
                summary.clone(),
            ))?;
        }
        let snapshot = JobProgressSnapshot::new(
            job.id,
            job.class,
            job.priority,
            label,
            job.volume,
            total_units.max(1),
        )
        .with_progress(JobProgressState::Planned, 0, summary, job_timestamp_ms());
        let progress_store = runtime_progress_store();
        if let Some(store) = &progress_store {
            let _access = preflight_runtime_write(store.path(), label)?;
            store.upsert(snapshot.clone())?;
        }
        Ok(Self {
            progress_store,
            last_progress: Arc::new(Mutex::new(snapshot)),
        })
    }

    pub(crate) fn running(&self) -> Result<()> {
        let detail = self
            .last_progress
            .lock()
            .expect("runtime job progress lock poisoned")
            .detail
            .clone();
        self.persist_progress(JobProgressState::Running, 0, detail)
    }

    pub(crate) fn deferred(&self, action: SchedulingAction) -> Result<()> {
        self.persist_progress(
            JobProgressState::Paused,
            0,
            format!("deferred:{}", action.as_str()),
        )
    }

    pub(crate) fn progress(
        &self,
        state: JobProgressState,
        completed_units: u64,
        detail: impl Into<String>,
    ) -> Result<()> {
        self.persist_progress(state, completed_units, detail)
    }

    pub(crate) fn resize(&self, total_units: u64, detail: impl Into<String>) -> Result<()> {
        let detail = detail.into();
        let mut last = self
            .last_progress
            .lock()
            .expect("runtime job progress lock poisoned");
        let mut snapshot = last.clone();
        snapshot.total_units = total_units.max(1);
        snapshot.completed_units = snapshot.completed_units.min(snapshot.total_units);
        snapshot.detail = detail;
        snapshot.updated_ms = job_timestamp_ms();
        if progress_semantics_equal(&last, &snapshot) {
            return Ok(());
        }

        if let Some(store) = &self.progress_store {
            let _access = preflight_runtime_write(store.path(), &snapshot.label)?;
            store.upsert(snapshot.clone())?;
        }
        *last = snapshot;
        Ok(())
    }

    pub(crate) fn finish(&self, status: &TaskStatus) -> Result<()> {
        let total_units = self
            .last_progress
            .lock()
            .expect("runtime job progress lock poisoned")
            .total_units;
        let (completed_units, detail) = match status {
            TaskStatus::Started => (0, "still-running".to_string()),
            TaskStatus::Completed => (total_units, "completed".to_string()),
            TaskStatus::Cancelled => (0, "cancelled".to_string()),
            TaskStatus::Failed(message) => (0, message.clone()),
        };
        self.persist_progress(JobProgressState::from(status), completed_units, detail)
    }

    fn persist_progress(
        &self,
        state: JobProgressState,
        completed_units: u64,
        detail: impl Into<String>,
    ) -> Result<()> {
        let Some(store) = &self.progress_store else {
            return Ok(());
        };
        let mut last = self
            .last_progress
            .lock()
            .expect("runtime job progress lock poisoned");
        let snapshot =
            last.clone()
                .with_progress(state, completed_units, detail, job_timestamp_ms());
        if progress_semantics_equal(&last, &snapshot) {
            return Ok(());
        }

        let _access = preflight_runtime_write(store.path(), &snapshot.label)?;
        store.upsert(snapshot.clone())?;
        *last = snapshot;
        Ok(())
    }
}

fn progress_semantics_equal(left: &JobProgressSnapshot, right: &JobProgressSnapshot) -> bool {
    left.id == right.id
        && left.class == right.class
        && left.priority == right.priority
        && left.label == right.label
        && left.volume == right.volume
        && left.state == right.state
        && left.completed_units == right.completed_units
        && left.total_units == right.total_units
        && left.detail == right.detail
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_jobs::{JobClass, JobId};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn progress_semantics_ignore_timestamp_only_drift() {
        let first = sample_progress_snapshot(JobId::from_raw(7)).with_progress(
            JobProgressState::Running,
            1,
            "copying",
            100,
        );
        let second = sample_progress_snapshot(JobId::from_raw(7)).with_progress(
            JobProgressState::Running,
            1,
            "copying",
            200,
        );

        assert!(progress_semantics_equal(&first, &second));
    }

    #[test]
    fn runtime_progress_skips_repeated_semantic_update_across_clones() {
        let path = temp_path("gfm-runtime-progress-noop", "gfmprogress");
        let store = JobProgressStore::new(&path);
        let initial = sample_progress_snapshot(JobId::from_raw(11)).with_progress(
            JobProgressState::Planned,
            0,
            "foreground:copy",
            10,
        );
        store.write_all(std::slice::from_ref(&initial)).unwrap();
        let handle = RuntimeJobHandle {
            progress_store: Some(store.clone()),
            last_progress: Arc::new(Mutex::new(initial)),
        };
        let cloned = handle.clone();

        handle.running().unwrap();
        let before_noop = std::fs::metadata(&path).unwrap().modified().unwrap();

        cloned.running().unwrap();

        let after_noop = std::fs::metadata(&path).unwrap().modified().unwrap();
        let snapshots = store.read().unwrap();
        assert_eq!(before_noop, after_noop);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].state, JobProgressState::Running);

        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn runtime_progress_resize_updates_finish_total_units() {
        let path = temp_path("gfm-runtime-progress-resize", "gfmprogress");
        let store = JobProgressStore::new(&path);
        let initial = sample_progress_snapshot(JobId::from_raw(12)).with_progress(
            JobProgressState::Planned,
            0,
            "index:/workspace",
            10,
        );
        store.write_all(std::slice::from_ref(&initial)).unwrap();
        let handle = RuntimeJobHandle {
            progress_store: Some(store.clone()),
            last_progress: Arc::new(Mutex::new(initial)),
        };

        handle.resize(42, "index:/workspace").unwrap();
        handle.finish(&TaskStatus::Completed).unwrap();

        let snapshots = store.read().unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].state, JobProgressState::Completed);
        assert_eq!(snapshots[0].completed_units, 42);
        assert_eq!(snapshots[0].total_units, 42);

        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn operation_conflict_store_round_trips_escaped_fields() {
        let path = temp_path("gfm-operation-conflict-escaped", "tsv");
        let store = OperationConflictStore::new(&path);
        let conflict = RuntimeOperationConflict {
            operation: "copy".to_string(),
            source: "/tmp/source\twith\ncontrols.md".to_string(),
            target: "/tmp/target\\literal\rname.md".to_string(),
            target_kind: "file".to_string(),
            selected_policy: "fail".to_string(),
            available_policies: vec!["replace".to_string(), "keep-both".to_string()],
            blocks_operation: true,
            reason: "destination\tconflict\nrequires\\choice".to_string(),
        };

        store.write_all(std::slice::from_ref(&conflict)).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            raw.contains("source=/tmp/source\\twith\\ncontrols.md"),
            "{raw}"
        );
        assert!(
            raw.contains("target=/tmp/target\\\\literal\\rname.md"),
            "{raw}"
        );
        assert_eq!(store.read().unwrap(), vec![conflict]);

        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn operation_conflict_store_preserves_legacy_literal_backslash_fields() {
        let path = temp_path("gfm-operation-conflict-legacy-backslash", "tsv");
        std::fs::write(
            &path,
            "operation-conflict\toperation=copy\tsource=/tmp/source\\x.md\ttarget=/tmp/target.md\texists=true\tkind=file\tpolicy=fail\tavailable=replace,keep-both\tblocks-operation=true\treason=needs-choice\n",
        )
        .unwrap();
        let store = OperationConflictStore::new(&path);

        let conflicts = store.read().unwrap();

        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].source, "/tmp/source\\x.md");

        std::fs::remove_file(path).unwrap();
    }

    fn sample_progress_snapshot(id: JobId) -> JobProgressSnapshot {
        JobProgressSnapshot::new(
            id,
            JobClass::Foreground,
            Priority::Interactive,
            "copy selected files",
            Some(VolumeId(2)),
            10,
        )
    }

    fn temp_path(prefix: &str, extension: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{}-{}-{}.{}",
            prefix,
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::SeqCst),
            extension
        ))
    }
}

fn preflight_runtime_write(path: &Path, worker: &str) -> Result<ScopedAccessGuard> {
    preflight_access_scope(write_probe_path(path), AccessIntent::Write, worker)
}

pub(crate) fn default_journal_path() -> PathBuf {
    env::var_os("GFM_OPS_JOURNAL")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir().join("gfm-ops.journal"))
}

pub(crate) fn default_trash_metadata_path() -> PathBuf {
    env::var_os("GFM_TRASH_METADATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir().join("gfm-trash.tsv"))
}

pub(crate) fn default_job_journal_path() -> PathBuf {
    env::var_os("GFM_JOB_JOURNAL")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir().join("gfm-jobs.journal"))
}

pub(crate) fn default_content_job_path() -> PathBuf {
    env::var_os("GFM_CONTENT_JOB")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir().join("gfm-content.job"))
}

pub(crate) fn default_extraction_quarantine_path() -> PathBuf {
    env::var_os("GFM_EXTRACTION_QUARANTINE")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir().join("gfm-extraction.gfmquarantine"))
}

pub(crate) fn default_security_bookmarks_path() -> PathBuf {
    env::var_os("GFM_SECURITY_BOOKMARKS")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir().join("gfm-security-bookmarks.tsv"))
}

pub(crate) fn default_permission_state_path() -> PathBuf {
    env::var_os("GFM_PERMISSION_STATE")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir().join("gfm-permission-state.tsv"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeOperationConflict {
    pub(crate) operation: String,
    pub(crate) source: String,
    pub(crate) target: String,
    pub(crate) target_kind: String,
    pub(crate) selected_policy: String,
    pub(crate) available_policies: Vec<String>,
    pub(crate) blocks_operation: bool,
    pub(crate) reason: String,
}

#[derive(Debug, Clone)]
pub(crate) struct OperationConflictStore {
    path: PathBuf,
}

impl OperationConflictStore {
    pub(crate) fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn append(&self, report: &OperationConflictReport) -> Result<()> {
        let _access = preflight_operation_conflict_write(&self.path)?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|err| GfmError::io(&self.path, err))?;
        writeln!(file, "{}", RuntimeOperationConflict::from(report).as_tsv())
            .map_err(|err| GfmError::io(&self.path, err))
    }

    pub(crate) fn read(&self) -> Result<Vec<RuntimeOperationConflict>> {
        let _access = preflight_operation_conflict_read(&self.path)?;
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(GfmError::io(&self.path, err)),
        };
        text.lines()
            .enumerate()
            .filter(|(_, line)| !line.trim().is_empty())
            .map(|(line_index, line)| parse_operation_conflict_line(&self.path, line_index, line))
            .collect()
    }

    pub(crate) fn resolve(
        &self,
        target: &str,
        selected_policy: &str,
    ) -> Result<RuntimeOperationConflict> {
        let resolved =
            self.resolve_target_set(BTreeSet::from([target.to_string()]), selected_policy)?;
        resolved.into_iter().next().ok_or_else(|| {
            GfmError::Format(format!(
                "operation conflict store {} has no blocking conflict for `{target}`",
                self.path.display()
            ))
        })
    }

    pub(crate) fn resolve_targets(
        &self,
        targets: &[String],
        selected_policy: &str,
    ) -> Result<Vec<RuntimeOperationConflict>> {
        self.resolve_target_set(targets.iter().cloned().collect(), selected_policy)
    }

    fn resolve_target_set(
        &self,
        targets: BTreeSet<String>,
        selected_policy: &str,
    ) -> Result<Vec<RuntimeOperationConflict>> {
        if targets.is_empty() {
            return Ok(Vec::new());
        }
        let mut conflicts = self.read()?;
        let mut unresolved_targets = targets.clone();
        let mut resolved = Vec::new();
        for conflict in conflicts.iter_mut() {
            if !targets.contains(&conflict.target) || !conflict.blocks_operation {
                continue;
            }
            unresolved_targets.remove(&conflict.target);
            if !conflict
                .available_policies
                .iter()
                .any(|policy| policy == selected_policy)
            {
                return Err(GfmError::Format(format!(
                    "operation conflict for `{}` cannot resolve with `{selected_policy}`; available={}",
                    conflict.target,
                    conflict.available_policies.join(",")
                )));
            }
            conflict.selected_policy = selected_policy.to_string();
            conflict.blocks_operation = false;
            conflict.reason = format!("destination-conflict-resolved-by-{selected_policy}");
            resolved.push(conflict.clone());
        }
        if let Some(target) = unresolved_targets.into_iter().next() {
            return Err(GfmError::Format(format!(
                "operation conflict store {} has no blocking conflict for `{target}`",
                self.path.display()
            )));
        }
        self.write_all(&conflicts)?;
        Ok(resolved)
    }

    fn write_all(&self, conflicts: &[RuntimeOperationConflict]) -> Result<()> {
        let _access = preflight_operation_conflict_write(&self.path)?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
        }
        let text = conflicts
            .iter()
            .map(RuntimeOperationConflict::as_tsv)
            .collect::<Vec<_>>()
            .join("\n");
        let text = if text.is_empty() {
            String::new()
        } else {
            format!("{text}\n")
        };
        let temporary = operation_conflict_temp_path(&self.path);
        fs::write(&temporary, text).map_err(|err| GfmError::io(&temporary, err))?;
        if let Err(err) = fs::rename(&temporary, &self.path) {
            let _ = fs::remove_file(&temporary);
            return Err(GfmError::io(&self.path, err));
        }
        Ok(())
    }
}

fn preflight_operation_conflict_read(path: &Path) -> Result<ScopedAccessGuard> {
    preflight_access_scope(path, AccessIntent::Read, "operation conflict store")
}

fn preflight_operation_conflict_write(path: &Path) -> Result<ScopedAccessGuard> {
    preflight_access_scope(
        write_probe_path(path),
        AccessIntent::Write,
        "operation conflict store",
    )
}

impl RuntimeOperationConflict {
    pub(crate) fn as_tsv(&self) -> String {
        format!(
            "operation-conflict\toperation={}\tsource={}\ttarget={}\texists={}\tkind={}\tpolicy={}\tavailable={}\tblocks-operation={}\treason={}",
            escape_field(&self.operation),
            escape_field(&self.source),
            escape_field(&self.target),
            self.target_kind != "none",
            escape_field(&self.target_kind),
            escape_field(&self.selected_policy),
            escape_field(&self.available_policies.join(",")),
            self.blocks_operation,
            escape_field(&self.reason)
        )
    }
}

impl From<&OperationConflictReport> for RuntimeOperationConflict {
    fn from(report: &OperationConflictReport) -> Self {
        Self {
            operation: report.operation.to_string(),
            source: report
                .source
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string()),
            target: report
                .target
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string()),
            target_kind: report.target_kind.as_str().to_string(),
            selected_policy: report.selected_policy.as_str().to_string(),
            available_policies: report
                .available_policies
                .iter()
                .map(|policy| policy.as_str().to_string())
                .collect(),
            blocks_operation: report.blocks_operation,
            reason: report.reason.clone(),
        }
    }
}

fn operation_conflict_temp_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("operation-conflicts.tsv");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    path.with_file_name(format!(".{file_name}.{}.{nonce}.tmp", std::process::id()))
}

pub(crate) fn runtime_operation_conflict_store() -> Option<OperationConflictStore> {
    env::var_os("GFM_OPERATION_CONFLICT_STORE").map(OperationConflictStore::new)
}

fn runtime_payload_catalog() -> Option<JobPayloadCatalog> {
    env::var_os("GFM_JOB_PAYLOAD_CATALOG").map(JobPayloadCatalog::new)
}

pub(crate) fn runtime_progress_store() -> Option<JobProgressStore> {
    env::var_os("GFM_JOB_PROGRESS_STORE").map(JobProgressStore::new)
}

pub(crate) fn preflight_runtime_job_state(worker: &str) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = Vec::new();
    if let Some(catalog) = runtime_payload_catalog() {
        guards.push(preflight_runtime_write(catalog.path(), worker)?);
    }
    if let Some(store) = runtime_progress_store() {
        guards.push(preflight_runtime_write(store.path(), worker)?);
    }
    Ok(guards)
}

fn runtime_payload_path(kind: JobPayloadKind, label: &str) -> PathBuf {
    PathBuf::from("runtime")
        .join(kind.as_str())
        .join(format!("{}.gfmjob", label_slug(label)))
}

fn write_probe_path(path: &Path) -> &Path {
    if path.is_dir() {
        return path;
    }
    path.parent().unwrap_or(path)
}

fn label_slug(label: &str) -> String {
    let mut slug = String::with_capacity(label.len());
    let mut last_dash = false;
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "job".to_string()
    } else {
        slug.to_string()
    }
}

fn parse_operation_conflict_line(
    path: &Path,
    line_index: usize,
    line: &str,
) -> Result<RuntimeOperationConflict> {
    let mut fields = line.split('\t');
    if fields.next() != Some("operation-conflict") {
        return Err(GfmError::Format(format!(
            "{}:{} expected operation-conflict record",
            path.display(),
            line_index + 1
        )));
    }
    let pairs = fields
        .filter_map(|field| field.split_once('='))
        .collect::<std::collections::BTreeMap<_, _>>();
    let value = |key: &str| -> Result<String> {
        pairs
            .get(key)
            .map(|value| unescape_field(value))
            .ok_or_else(|| {
                GfmError::Format(format!(
                    "{}:{} missing operation-conflict `{key}` field",
                    path.display(),
                    line_index + 1
                ))
            })?
    };
    Ok(RuntimeOperationConflict {
        operation: value("operation")?,
        source: pairs
            .get("source")
            .map(|value| unescape_field(value))
            .transpose()?
            .unwrap_or_else(|| "-".to_string()),
        target: value("target")?,
        target_kind: value("kind")?,
        selected_policy: value("policy")?,
        available_policies: value("available")?
            .split(',')
            .filter(|policy| !policy.is_empty())
            .map(str::to_string)
            .collect(),
        blocks_operation: match value("blocks-operation")?.as_str() {
            "true" => true,
            "false" => false,
            other => {
                return Err(GfmError::Format(format!(
                    "{}:{} invalid operation-conflict blocks-operation `{other}`",
                    path.display(),
                    line_index + 1
                )));
            }
        },
        reason: value("reason")?,
    })
}

fn escape_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn unescape_field(value: &str) -> Result<String> {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars();
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
            Some(other) => {
                output.push('\\');
                output.push(other);
            }
            None => {
                output.push('\\');
            }
        }
    }
    Ok(output)
}

fn payload_kind_for_label(label: &str) -> JobPayloadKind {
    let label = label.to_ascii_lowercase();
    if label.contains("thumbnail") {
        JobPayloadKind::Thumbnail
    } else if label.contains("quicklook") || label.contains("preview") {
        JobPayloadKind::Preview
    } else if label.contains("repair") || label.contains("recover") {
        JobPayloadKind::Repair
    } else if label.contains("extract") {
        JobPayloadKind::Extraction
    } else if label.contains("index") || label.contains("content") {
        JobPayloadKind::Indexing
    } else {
        JobPayloadKind::Operation
    }
}

fn job_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}
