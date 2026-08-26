use gfm_jobs::{
    Cancellation, Job, JobJournal, JobPayloadCatalog, JobPayloadKind, JobPayloadRecord,
    JobProgressSnapshot, JobProgressState, JobProgressStore, Priority, RetriableTask, RetryPolicy,
    Scheduler, SchedulingAction, SchedulingPressure, Task, TaskStatus, VolumeConcurrencyPolicy,
    WorkerPool,
};
use gfm_types::{GfmError, Result, VolumeId};
use std::env;
use std::path::PathBuf;
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
    let result_slot = Arc::new(Mutex::new(None));
    let result_slot_task = Arc::clone(&result_slot);
    let mut scheduler = Scheduler::new();
    let job = if let Some(volume) = volume {
        scheduler.schedule_on_volume(priority, label, volume)
    } else {
        scheduler.schedule(priority, label)
    };
    let runtime = RuntimeJobHandle::begin(
        &job,
        payload_kind_for_label(label),
        label,
        1,
        format!("{}:{label}", priority.as_str()),
    )?;
    let runtime_task = runtime.clone();
    let task = Task::new(job.clone(), move |_| {
        runtime_task.running()?;
        let result = work()?;
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
    let scheduling = pressure.decide(priority, 1, 1);
    if scheduling.action == SchedulingAction::Defer {
        return Ok(ScheduledTaskOutcome {
            result: None,
            scheduling_action: scheduling.action,
            deferred: true,
        });
    }

    let result_slot = Arc::new(Mutex::new(None));
    let result_slot_task = Arc::clone(&result_slot);
    let mut scheduler = Scheduler::new();
    let job = if let Some(volume) = volume {
        scheduler.schedule_on_volume(priority, label, volume)
    } else {
        scheduler.schedule(priority, label)
    };
    let runtime = RuntimeJobHandle::begin(
        &job,
        payload_kind_for_label(label),
        label,
        1,
        format!("{}:{label}:adaptive", priority.as_str()),
    )?;
    let runtime_task = runtime.clone();
    let task = RetriableTask::new(job.clone(), move |cancellation| {
        runtime_task.running()?;
        let result = work(cancellation)?;
        *result_slot_task
            .lock()
            .expect("scheduled task result lock poisoned") = Some(result);
        Ok(())
    });
    let journal = JobJournal::new(default_job_journal_path());
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

#[derive(Clone)]
pub(crate) struct RuntimeJobHandle {
    progress_store: Option<JobProgressStore>,
    snapshot: JobProgressSnapshot,
}

impl RuntimeJobHandle {
    pub(crate) fn begin(
        job: &Job,
        kind: JobPayloadKind,
        label: &str,
        total_units: u64,
        summary: String,
    ) -> Result<Self> {
        if let Some(catalog) = runtime_payload_catalog() {
            catalog.append(&JobPayloadRecord::new(
                job.id,
                kind,
                label,
                runtime_payload_path(kind, label),
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
            store.upsert(snapshot.clone())?;
        }
        Ok(Self {
            progress_store,
            snapshot,
        })
    }

    pub(crate) fn running(&self) -> Result<()> {
        if let Some(store) = &self.progress_store {
            store.upsert(self.snapshot.clone().with_progress(
                JobProgressState::Running,
                0,
                self.snapshot.detail.clone(),
                job_timestamp_ms(),
            ))?;
        }
        Ok(())
    }

    pub(crate) fn finish(&self, status: &TaskStatus) -> Result<()> {
        if let Some(store) = &self.progress_store {
            let (completed_units, detail) = match status {
                TaskStatus::Started => (0, "still-running".to_string()),
                TaskStatus::Completed => (self.snapshot.total_units, "completed".to_string()),
                TaskStatus::Cancelled => (0, "cancelled".to_string()),
                TaskStatus::Failed(message) => (0, message.clone()),
            };
            store.upsert(self.snapshot.clone().with_progress(
                JobProgressState::from(status),
                completed_units,
                detail,
                job_timestamp_ms(),
            ))?;
        }
        Ok(())
    }
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

fn runtime_payload_catalog() -> Option<JobPayloadCatalog> {
    env::var_os("GFM_JOB_PAYLOAD_CATALOG").map(JobPayloadCatalog::new)
}

fn runtime_progress_store() -> Option<JobProgressStore> {
    env::var_os("GFM_JOB_PROGRESS_STORE").map(JobProgressStore::new)
}

fn runtime_payload_path(kind: JobPayloadKind, label: &str) -> PathBuf {
    PathBuf::from("runtime")
        .join(kind.as_str())
        .join(format!("{}.gfmjob", label_slug(label)))
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
