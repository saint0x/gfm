use gfm_types::{GfmError, Result, VolumeId};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

static PAYLOAD_CATALOG_TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

mod cancel;
mod fair;
mod isolated;
mod progress;
mod retry;
mod schedule;
pub use cancel::Cancellation;
pub use fair::{BlockedJob, JobFairnessPlan, JobFairnessPlanner, JobFairnessPolicy};
use isolated::{IsolatedRetriableTaskQueue, IsolatedTaskQueue};
pub use progress::{JobProgressCommand, JobProgressSnapshot, JobProgressState, JobProgressStore};
pub use retry::{FailureClass, RetryDecision, RetryPolicy};
pub use schedule::{
    JobBatteryState, JobIoPressure, JobThermalState, JobUserActivity, SchedulingAction,
    SchedulingDecision, SchedulingPressure, VolumeConcurrencyPolicy,
};

fn path_exists(path: &Path, context: &str) -> Result<bool> {
    path.try_exists()
        .map_err(|err| GfmError::io(path, format!("{context} existence unavailable: {err}")))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct JobId(u64);

impl JobId {
    pub const fn value(self) -> u64 {
        self.0
    }

    pub const fn from_raw(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Background,
    Normal,
    Visible,
    Interactive,
}

impl Priority {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::Normal => "normal",
            Self::Visible => "visible",
            Self::Interactive => "interactive",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "background" => Some(Self::Background),
            "normal" => Some(Self::Normal),
            "visible" => Some(Self::Visible),
            "interactive" => Some(Self::Interactive),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JobClass {
    Foreground,
    Visible,
    Background,
    Maintenance,
    Repair,
}

impl JobClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Foreground => "foreground",
            Self::Visible => "visible",
            Self::Background => "background",
            Self::Maintenance => "maintenance",
            Self::Repair => "repair",
        }
    }

    pub const fn from_priority(priority: Priority) -> Self {
        match priority {
            Priority::Interactive => Self::Foreground,
            Priority::Visible => Self::Visible,
            Priority::Normal | Priority::Background => Self::Background,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "foreground" => Some(Self::Foreground),
            "visible" => Some(Self::Visible),
            "background" => Some(Self::Background),
            "maintenance" => Some(Self::Maintenance),
            "repair" => Some(Self::Repair),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: JobId,
    pub priority: Priority,
    pub class: JobClass,
    pub label: String,
    pub volume: Option<VolumeId>,
    pub dependencies: Vec<JobId>,
    cancel: Cancellation,
}

impl Job {
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn cancellation(&self) -> Cancellation {
        self.cancel.clone()
    }
}

#[derive(Debug, Default)]
pub struct Scheduler {
    next: AtomicU64,
    queue: BinaryHeap<QueuedJob>,
    cancelled: HashSet<JobId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeCancellationReport {
    pub volume: VolumeId,
    pub class: Option<JobClass>,
    pub cancelled: Vec<CancelledJob>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelledJob {
    pub id: JobId,
    pub label: String,
    pub class: JobClass,
    pub priority: Priority,
}

impl VolumeCancellationReport {
    pub fn as_tsv(&self) -> String {
        let header = format!(
            "volume-job-cancellation\tvolume={}\tclass={}\tcancelled={}",
            self.volume.0,
            self.class.map(JobClass::as_str).unwrap_or("-"),
            self.cancelled.len()
        );
        if self.cancelled.is_empty() {
            return header;
        }
        let jobs = self
            .cancelled
            .iter()
            .map(|job| {
                format!(
                    "cancelled-job\t{}\t{}\t{}\t{}",
                    job.id.value(),
                    job.class.as_str(),
                    job.priority.as_str(),
                    escape(&job.label)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!("{header}\n{jobs}")
    }
}

impl Scheduler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn schedule(&mut self, priority: Priority, label: impl Into<String>) -> Job {
        self.schedule_with_volume(priority, label, None)
    }

    pub fn schedule_in_class(
        &mut self,
        priority: Priority,
        class: JobClass,
        label: impl Into<String>,
    ) -> Job {
        self.schedule_with_options(priority, class, label, None, Vec::new())
    }

    pub fn schedule_with_dependencies(
        &mut self,
        priority: Priority,
        label: impl Into<String>,
        dependencies: impl IntoIterator<Item = JobId>,
    ) -> Job {
        self.schedule_with_options(
            priority,
            JobClass::from_priority(priority),
            label,
            None,
            dependencies.into_iter().collect(),
        )
    }

    pub fn schedule_in_class_with_dependencies(
        &mut self,
        priority: Priority,
        class: JobClass,
        label: impl Into<String>,
        dependencies: impl IntoIterator<Item = JobId>,
    ) -> Job {
        self.schedule_with_options(
            priority,
            class,
            label,
            None,
            dependencies.into_iter().collect(),
        )
    }

    pub fn schedule_on_volume(
        &mut self,
        priority: Priority,
        label: impl Into<String>,
        volume: VolumeId,
    ) -> Job {
        self.schedule_with_volume(priority, label, Some(volume))
    }

    pub fn schedule_on_volume_in_class(
        &mut self,
        priority: Priority,
        class: JobClass,
        label: impl Into<String>,
        volume: VolumeId,
    ) -> Job {
        self.schedule_with_options(priority, class, label, Some(volume), Vec::new())
    }

    pub fn schedule_on_volume_with_dependencies(
        &mut self,
        priority: Priority,
        label: impl Into<String>,
        volume: VolumeId,
        dependencies: impl IntoIterator<Item = JobId>,
    ) -> Job {
        self.schedule_with_options(
            priority,
            JobClass::from_priority(priority),
            label,
            Some(volume),
            dependencies.into_iter().collect(),
        )
    }

    pub fn schedule_on_volume_in_class_with_dependencies(
        &mut self,
        priority: Priority,
        class: JobClass,
        label: impl Into<String>,
        volume: VolumeId,
        dependencies: impl IntoIterator<Item = JobId>,
    ) -> Job {
        self.schedule_with_options(
            priority,
            class,
            label,
            Some(volume),
            dependencies.into_iter().collect(),
        )
    }

    fn schedule_with_volume(
        &mut self,
        priority: Priority,
        label: impl Into<String>,
        volume: Option<VolumeId>,
    ) -> Job {
        self.schedule_with_options(
            priority,
            JobClass::from_priority(priority),
            label,
            volume,
            Vec::new(),
        )
    }

    fn schedule_with_options(
        &mut self,
        priority: Priority,
        class: JobClass,
        label: impl Into<String>,
        volume: Option<VolumeId>,
        dependencies: Vec<JobId>,
    ) -> Job {
        let id = JobId(self.next.fetch_add(1, AtomicOrdering::SeqCst) + 1);
        let job = Job {
            id,
            priority,
            class,
            label: label.into(),
            volume,
            dependencies,
            cancel: Cancellation::default(),
        };
        self.queue.push(QueuedJob(job.clone()));
        job
    }

    pub fn cancel(&mut self, id: JobId) {
        self.cancelled.insert(id);
    }

    pub fn bind_volume(&mut self, id: JobId, volume: VolumeId) -> Option<Job> {
        let mut retained = BinaryHeap::new();
        let mut rebound = None;
        while let Some(QueuedJob(mut job)) = self.queue.pop() {
            if job.id == id {
                job.volume = Some(volume);
                rebound = Some(job.clone());
            }
            retained.push(QueuedJob(job));
        }
        self.queue = retained;
        rebound
    }

    pub fn cancel_volume_jobs(
        &mut self,
        volume: VolumeId,
        class: Option<JobClass>,
    ) -> VolumeCancellationReport {
        let mut retained = BinaryHeap::new();
        let mut cancelled = Vec::new();
        while let Some(QueuedJob(job)) = self.queue.pop() {
            if job.volume == Some(volume) && class.is_none_or(|class| class == job.class) {
                self.cancelled.remove(&job.id);
                job.cancel();
                cancelled.push(CancelledJob {
                    id: job.id,
                    label: job.label,
                    class: job.class,
                    priority: job.priority,
                });
            } else {
                retained.push(QueuedJob(job));
            }
        }
        self.queue = retained;
        cancelled.sort_by_key(|job| job.id.value());
        VolumeCancellationReport {
            volume,
            class,
            cancelled,
        }
    }

    pub fn pop_next(&mut self) -> Option<Job> {
        while let Some(QueuedJob(job)) = self.queue.pop() {
            if self.cancelled.remove(&job.id) {
                job.cancel();
                continue;
            }
            return Some(job);
        }
        None
    }

    pub fn drain_ready(&mut self) -> Vec<Job> {
        let mut jobs = Vec::new();
        while let Some(job) = self.pop_next() {
            jobs.push(job);
        }
        jobs
    }

    pub fn drain_fair_ready(
        &mut self,
        policy: JobFairnessPolicy,
        completed: impl IntoIterator<Item = JobId>,
    ) -> JobFairnessPlan {
        let drained = self.drain_ready();
        let plan = JobFairnessPlanner::new(policy)
            .with_completed(completed)
            .plan(drained.clone());
        let blocked_ids = plan
            .blocked
            .iter()
            .map(|job| job.id)
            .collect::<HashSet<_>>();
        for job in drained
            .into_iter()
            .filter(|job| blocked_ids.contains(&job.id))
        {
            self.queue.push(QueuedJob(job));
        }
        plan
    }
}

pub struct Task {
    job: Job,
    work: Option<Box<dyn FnOnce(Cancellation) -> Result<()> + Send + 'static>>,
}

impl Task {
    pub fn new(job: Job, work: impl FnOnce(Cancellation) -> Result<()> + Send + 'static) -> Self {
        Self {
            job,
            work: Some(Box::new(work)),
        }
    }
}

#[derive(Clone)]
pub struct RetriableTask {
    job: Job,
    work: Arc<dyn Fn(Cancellation) -> Result<()> + Send + Sync + 'static>,
}

impl RetriableTask {
    pub fn new(
        job: Job,
        work: impl Fn(Cancellation) -> Result<()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            job,
            work: Arc::new(work),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskOutcome {
    pub id: JobId,
    pub label: String,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    Started,
    Completed,
    Cancelled,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerReport {
    pub outcomes: Vec<TaskOutcome>,
}

impl WorkerReport {
    pub fn completed(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| outcome.status == TaskStatus::Completed)
            .count()
    }

    pub fn cancelled(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| outcome.status == TaskStatus::Cancelled)
            .count()
    }

    pub fn failed(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| matches!(outcome.status, TaskStatus::Failed(_)))
            .count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    pub id: JobId,
    pub label: String,
    pub attempt: usize,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryJob {
    pub id: JobId,
    pub label: String,
    pub attempts: usize,
    pub reason: RecoveryReason,
    pub failure_class: Option<FailureClass>,
    pub next_delay_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryReason {
    Interrupted,
    RetryableFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobPayloadKind {
    Operation,
    Indexing,
    Extraction,
    Thumbnail,
    Preview,
    Repair,
}

impl JobPayloadKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Operation => "operation",
            Self::Indexing => "indexing",
            Self::Extraction => "extraction",
            Self::Thumbnail => "thumbnail",
            Self::Preview => "preview",
            Self::Repair => "repair",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "operation" => Some(Self::Operation),
            "indexing" => Some(Self::Indexing),
            "extraction" => Some(Self::Extraction),
            "thumbnail" => Some(Self::Thumbnail),
            "preview" => Some(Self::Preview),
            "repair" => Some(Self::Repair),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobPayloadRecord {
    pub id: JobId,
    pub kind: JobPayloadKind,
    pub label: String,
    pub payload_path: PathBuf,
    pub volume: Option<VolumeId>,
    pub summary: String,
}

impl JobPayloadRecord {
    pub fn new(
        id: JobId,
        kind: JobPayloadKind,
        label: impl Into<String>,
        payload_path: impl Into<PathBuf>,
        volume: Option<VolumeId>,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            id,
            kind,
            label: label.into(),
            payload_path: payload_path.into(),
            volume,
            summary: summary.into(),
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "payload\t{}\t{}\t{}\t{}\t{}\t{}",
            self.id.value(),
            self.kind.as_str(),
            escape(&self.label),
            escape(&self.payload_path.to_string_lossy()),
            self.volume
                .map(|volume| volume.0.to_string())
                .unwrap_or_else(|| "-".to_string()),
            escape(&self.summary)
        )
    }
}

#[derive(Debug, Clone)]
pub struct JobPayloadCatalog {
    path: PathBuf,
}

impl JobPayloadCatalog {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write_all(&self, records: &[JobPayloadRecord]) -> Result<()> {
        let parent = real_parent_or_cwd(&self.path);
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
        let temporary = self.temp_path();
        {
            let file = File::create(&temporary).map_err(|err| GfmError::io(&temporary, err))?;
            let mut writer = BufWriter::new(file);
            writeln!(writer, "gfm-job-payload-catalog-v1")
                .map_err(|err| GfmError::io(&temporary, err))?;
            for record in records {
                writeln!(writer, "{}", record.as_tsv())
                    .map_err(|err| GfmError::io(&temporary, err))?;
            }
            writer
                .flush()
                .map_err(|err| GfmError::io(&temporary, err))?;
        }
        if let Err(err) = fs::rename(&temporary, &self.path) {
            let _ = fs::remove_file(&temporary);
            return Err(GfmError::io(&self.path, err));
        }
        Ok(())
    }

    pub fn append(&self, record: &JobPayloadRecord) -> Result<()> {
        if !path_exists(&self.path, "job payload catalog")? {
            self.write_all(&[])?;
        }
        if self.contains_id(record.id)? {
            let mut records = self.read()?;
            if let Some(existing) = records.iter_mut().find(|existing| existing.id == record.id) {
                if existing == record {
                    return Ok(());
                }
                *existing = record.clone();
            }
            self.write_all(&records)?;
            return Ok(());
        }
        let mut file = OpenOptions::new()
            .append(true)
            .open(&self.path)
            .map_err(|err| GfmError::io(&self.path, err))?;
        writeln!(file, "{}", record.as_tsv()).map_err(|err| GfmError::io(&self.path, err))
    }

    pub fn read(&self) -> Result<Vec<JobPayloadRecord>> {
        if !path_exists(&self.path, "job payload catalog")? {
            return Ok(Vec::new());
        }
        let file = File::open(&self.path).map_err(|err| GfmError::io(&self.path, err))?;
        let mut lines = BufReader::new(file).lines();
        let header = lines
            .next()
            .transpose()
            .map_err(|err| GfmError::io(&self.path, err))?
            .ok_or_else(|| {
                GfmError::Format(format!("empty payload catalog {}", self.path.display()))
            })?;
        if header != "gfm-job-payload-catalog-v1" {
            return Err(GfmError::Format(format!(
                "unsupported payload catalog header `{header}` in {}",
                self.path.display()
            )));
        }
        let mut records = Vec::new();
        for (line_index, line) in lines.enumerate() {
            let line = line.map_err(|err| GfmError::io(&self.path, err))?;
            records.push(parse_payload_record(&line).map_err(|err| {
                GfmError::Format(format!(
                    "{} line {}: {}",
                    self.path.display(),
                    line_index + 2,
                    err
                ))
            })?);
        }
        Ok(records)
    }

    pub fn read_for_ids(
        &self,
        ids: impl IntoIterator<Item = JobId>,
    ) -> Result<Vec<JobPayloadRecord>> {
        let wanted = ids.into_iter().collect::<HashSet<_>>();
        if wanted.is_empty() {
            return Ok(Vec::new());
        }
        if !path_exists(&self.path, "job payload catalog")? {
            return Ok(Vec::new());
        }
        let file = File::open(&self.path).map_err(|err| GfmError::io(&self.path, err))?;
        let mut lines = BufReader::new(file).lines();
        let header = lines
            .next()
            .transpose()
            .map_err(|err| GfmError::io(&self.path, err))?
            .ok_or_else(|| {
                GfmError::Format(format!("empty payload catalog {}", self.path.display()))
            })?;
        if header != "gfm-job-payload-catalog-v1" {
            return Err(GfmError::Format(format!(
                "unsupported payload catalog header `{header}` in {}",
                self.path.display()
            )));
        }
        let mut records = Vec::new();
        for (line_index, line) in lines.enumerate() {
            let line = line.map_err(|err| GfmError::io(&self.path, err))?;
            let record = parse_payload_record(&line).map_err(|err| {
                GfmError::Format(format!(
                    "{} line {}: {}",
                    self.path.display(),
                    line_index + 2,
                    err
                ))
            })?;
            if wanted.contains(&record.id) {
                if let Some(index) = records
                    .iter()
                    .position(|existing: &JobPayloadRecord| existing.id == record.id)
                {
                    records[index] = record;
                } else {
                    records.push(record);
                }
            }
        }
        Ok(records)
    }

    fn contains_id(&self, id: JobId) -> Result<bool> {
        if !path_exists(&self.path, "job payload catalog")? {
            return Ok(false);
        }
        let file = File::open(&self.path).map_err(|err| GfmError::io(&self.path, err))?;
        let mut lines = BufReader::new(file).lines();
        let header = lines
            .next()
            .transpose()
            .map_err(|err| GfmError::io(&self.path, err))?
            .ok_or_else(|| {
                GfmError::Format(format!("empty payload catalog {}", self.path.display()))
            })?;
        if header != "gfm-job-payload-catalog-v1" {
            return Err(GfmError::Format(format!(
                "unsupported payload catalog header `{header}` in {}",
                self.path.display()
            )));
        }
        for (line_index, line) in lines.enumerate() {
            let line = line.map_err(|err| GfmError::io(&self.path, err))?;
            let record = parse_payload_record(&line).map_err(|err| {
                GfmError::Format(format!(
                    "{} line {}: {}",
                    self.path.display(),
                    line_index + 2,
                    err
                ))
            })?;
            if record.id == id {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn temp_path(&self) -> PathBuf {
        let mut name = self
            .path
            .file_name()
            .map(|name| name.to_os_string())
            .unwrap_or_else(|| "job-payload-catalog".into());
        let sequence = PAYLOAD_CATALOG_TEMP_FILE_SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed);
        name.push(format!(".{}.{sequence}.tmp", std::process::id()));
        self.path.with_file_name(name)
    }
}

#[derive(Debug, Clone)]
pub struct JobJournal {
    path: PathBuf,
}

impl JobJournal {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, entry: &JournalEntry) -> Result<()> {
        let parent = real_parent_or_cwd(&self.path);
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|err| GfmError::io(&self.path, err))?;
        let mut writer = BufWriter::new(file);
        writeln!(
            writer,
            "{}\t{}\t{}\t{}",
            entry.id.value(),
            entry.attempt,
            encode_status(&entry.status),
            escape(&entry.label),
        )
        .map_err(|err| GfmError::io(&self.path, err))?;
        writer.flush().map_err(|err| GfmError::io(&self.path, err))
    }

    pub fn read(&self) -> Result<Vec<JournalEntry>> {
        if !path_exists(&self.path, "job journal")? {
            return Ok(Vec::new());
        }
        let file = File::open(&self.path).map_err(|err| GfmError::io(&self.path, err))?;
        let reader = BufReader::new(file);
        let mut entries = Vec::new();
        for (line_index, line) in reader.lines().enumerate() {
            let line = line.map_err(|err| GfmError::io(&self.path, err))?;
            entries.push(parse_journal_entry(&line).map_err(|err| {
                GfmError::Format(format!(
                    "{} line {}: {}",
                    self.path.display(),
                    line_index + 1,
                    err
                ))
            })?);
        }
        Ok(entries)
    }

    pub fn attempts_for(&self, id: JobId) -> Result<usize> {
        Ok(self
            .read()?
            .into_iter()
            .filter(|entry| entry.id == id && entry.status == TaskStatus::Started)
            .count())
    }

    pub fn recoverable(&self, policy: RetryPolicy) -> Result<Vec<RecoveryJob>> {
        let mut states: Vec<JobRecoveryState> = Vec::new();
        for entry in self.read()? {
            if let Some(state) = states.iter_mut().find(|state| state.id == entry.id) {
                state.apply(entry);
            } else {
                states.push(JobRecoveryState::from_entry(entry));
            }
        }

        let max_attempts = policy.max_attempts.max(1);
        let mut jobs = Vec::new();
        for state in states {
            if state.last_status == Some(TaskStatus::Started) {
                jobs.push(RecoveryJob {
                    id: state.id,
                    label: state.label,
                    attempts: state.attempts,
                    reason: RecoveryReason::Interrupted,
                    failure_class: None,
                    next_delay_ms: 0,
                });
            } else if let Some(TaskStatus::Failed(message)) = &state.last_status {
                let decision = policy.retry_decision(state.attempts, message);
                if decision.retryable && state.attempts < max_attempts {
                    jobs.push(RecoveryJob {
                        id: state.id,
                        label: state.label,
                        attempts: state.attempts,
                        reason: RecoveryReason::RetryableFailure,
                        failure_class: Some(decision.class),
                        next_delay_ms: decision.next_delay_ms,
                    });
                }
            }
        }
        jobs.sort_by_key(|job| job.id.value());
        Ok(jobs)
    }
}

#[derive(Debug, Clone)]
struct JobRecoveryState {
    id: JobId,
    label: String,
    attempts: usize,
    last_status: Option<TaskStatus>,
}

impl JobRecoveryState {
    fn from_entry(entry: JournalEntry) -> Self {
        let mut state = Self {
            id: entry.id,
            label: entry.label.clone(),
            attempts: 0,
            last_status: None,
        };
        state.apply(entry);
        state
    }

    fn apply(&mut self, entry: JournalEntry) {
        self.label = entry.label;
        self.attempts = self.attempts.max(entry.attempt);
        self.last_status = Some(entry.status);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerPool {
    threads: usize,
}

impl WorkerPool {
    pub fn new(threads: usize) -> Self {
        Self {
            threads: threads.max(1),
        }
    }

    pub fn run(&self, tasks: Vec<Task>) -> WorkerReport {
        if tasks.is_empty() {
            return WorkerReport {
                outcomes: Vec::new(),
            };
        }

        let task_count = tasks.len();
        let queue = Arc::new(Mutex::new(VecDeque::from(tasks)));
        let outcomes = Arc::new(Mutex::new(Vec::with_capacity(task_count)));
        let threads = self.threads.min(task_count);

        thread::scope(|scope| {
            for _ in 0..threads {
                let queue = Arc::clone(&queue);
                let outcomes = Arc::clone(&outcomes);
                scope.spawn(move || loop {
                    let task = {
                        let mut queue = queue.lock().expect("worker task queue poisoned");
                        queue.pop_front()
                    };
                    let Some(mut task) = task else {
                        break;
                    };
                    let cancellation = task.job.cancellation();
                    let result = cancellation.check().and_then(|()| {
                        task.work.take().expect("worker task missing work")(cancellation)
                    });
                    let status = match result {
                        Ok(()) => TaskStatus::Completed,
                        Err(GfmError::Cancelled) => TaskStatus::Cancelled,
                        Err(err) => TaskStatus::Failed(err.to_string()),
                    };
                    outcomes
                        .lock()
                        .expect("worker outcome list poisoned")
                        .push(TaskOutcome {
                            id: task.job.id,
                            label: task.job.label,
                            status,
                        });
                });
            }
        });

        let mut outcomes = Arc::try_unwrap(outcomes)
            .expect("worker outcomes still shared")
            .into_inner()
            .expect("worker outcome list poisoned");
        outcomes.sort_by_key(|outcome| outcome.id.value());
        WorkerReport { outcomes }
    }

    pub fn run_isolated(&self, tasks: Vec<Task>, policy: VolumeConcurrencyPolicy) -> WorkerReport {
        if tasks.is_empty() {
            return WorkerReport {
                outcomes: Vec::new(),
            };
        }

        let task_count = tasks.len();
        let queue = Arc::new(IsolatedTaskQueue::new(tasks, policy));
        let outcomes = Arc::new(Mutex::new(Vec::with_capacity(task_count)));
        let threads = self.threads.min(task_count);

        thread::scope(|scope| {
            for _ in 0..threads {
                let queue = Arc::clone(&queue);
                let outcomes = Arc::clone(&outcomes);
                scope.spawn(move || loop {
                    let Some(mut lease) = queue.next() else {
                        break;
                    };
                    let cancellation = lease.task.job.cancellation();
                    let result = cancellation.check().and_then(|()| {
                        lease.task.work.take().expect("worker task missing work")(cancellation)
                    });
                    let status = match result {
                        Ok(()) => TaskStatus::Completed,
                        Err(GfmError::Cancelled) => TaskStatus::Cancelled,
                        Err(err) => TaskStatus::Failed(err.to_string()),
                    };
                    let job = lease.finish();
                    outcomes
                        .lock()
                        .expect("worker outcome list poisoned")
                        .push(TaskOutcome {
                            id: job.id,
                            label: job.label,
                            status,
                        });
                });
            }
        });

        let mut outcomes = Arc::try_unwrap(outcomes)
            .expect("worker outcomes still shared")
            .into_inner()
            .expect("worker outcome list poisoned");
        outcomes.sort_by_key(|outcome| outcome.id.value());
        WorkerReport { outcomes }
    }

    pub fn run_retriable(
        &self,
        tasks: Vec<RetriableTask>,
        journal: &JobJournal,
        policy: RetryPolicy,
    ) -> WorkerReport {
        if tasks.is_empty() {
            return WorkerReport {
                outcomes: Vec::new(),
            };
        }

        let task_count = tasks.len();
        let queue = Arc::new(Mutex::new(VecDeque::from(tasks)));
        let outcomes = Arc::new(Mutex::new(Vec::with_capacity(task_count)));
        let threads = self.threads.min(task_count);

        thread::scope(|scope| {
            for _ in 0..threads {
                let queue = Arc::clone(&queue);
                let outcomes = Arc::clone(&outcomes);
                let journal = journal.clone();
                scope.spawn(move || loop {
                    let task = {
                        let mut queue = queue.lock().expect("worker task queue poisoned");
                        queue.pop_front()
                    };
                    let Some(task) = task else {
                        break;
                    };

                    let final_status = execute_retriable_task(&task, &journal, policy);

                    outcomes
                        .lock()
                        .expect("worker outcome list poisoned")
                        .push(TaskOutcome {
                            id: task.job.id,
                            label: task.job.label,
                            status: final_status,
                        });
                });
            }
        });

        let mut outcomes = Arc::try_unwrap(outcomes)
            .expect("worker outcomes still shared")
            .into_inner()
            .expect("worker outcome list poisoned");
        outcomes.sort_by_key(|outcome| outcome.id.value());
        WorkerReport { outcomes }
    }

    pub fn run_retriable_isolated(
        &self,
        tasks: Vec<RetriableTask>,
        journal: &JobJournal,
        retry_policy: RetryPolicy,
        volume_policy: VolumeConcurrencyPolicy,
    ) -> WorkerReport {
        if tasks.is_empty() {
            return WorkerReport {
                outcomes: Vec::new(),
            };
        }

        let task_count = tasks.len();
        let queue = Arc::new(IsolatedRetriableTaskQueue::new(tasks, volume_policy));
        let outcomes = Arc::new(Mutex::new(Vec::with_capacity(task_count)));
        let threads = self.threads.min(task_count);

        thread::scope(|scope| {
            for _ in 0..threads {
                let queue = Arc::clone(&queue);
                let outcomes = Arc::clone(&outcomes);
                let journal = journal.clone();
                scope.spawn(move || loop {
                    let Some(lease) = queue.next() else {
                        break;
                    };
                    let final_status = execute_retriable_task(&lease.task, &journal, retry_policy);
                    let job = lease.finish();
                    outcomes
                        .lock()
                        .expect("worker outcome list poisoned")
                        .push(TaskOutcome {
                            id: job.id,
                            label: job.label,
                            status: final_status,
                        });
                });
            }
        });

        let mut outcomes = Arc::try_unwrap(outcomes)
            .expect("worker outcomes still shared")
            .into_inner()
            .expect("worker outcome list poisoned");
        outcomes.sort_by_key(|outcome| outcome.id.value());
        WorkerReport { outcomes }
    }
}

fn execute_retriable_task(
    task: &RetriableTask,
    journal: &JobJournal,
    retry_policy: RetryPolicy,
) -> TaskStatus {
    let mut final_status = TaskStatus::Failed("task did not run".to_string());
    let attempts = retry_policy.max_attempts.max(1);
    let cancellation = task.job.cancellation();
    for attempt in 1..=attempts {
        if cancellation.is_cancelled() {
            return TaskStatus::Cancelled;
        }
        let started = JournalEntry {
            id: task.job.id,
            label: task.job.label.clone(),
            attempt,
            status: TaskStatus::Started,
        };
        if let Err(err) = journal.append(&started) {
            final_status = TaskStatus::Failed(err.to_string());
            break;
        }

        let status = match (task.work)(cancellation.clone()) {
            Ok(()) => TaskStatus::Completed,
            Err(GfmError::Cancelled) => TaskStatus::Cancelled,
            Err(err) => TaskStatus::Failed(err.to_string()),
        };
        let finished = JournalEntry {
            id: task.job.id,
            label: task.job.label.clone(),
            attempt,
            status: status.clone(),
        };
        if let Err(err) = journal.append(&finished) {
            final_status = TaskStatus::Failed(err.to_string());
            break;
        }

        final_status = status;
        if final_status != TaskStatus::Cancelled && !matches!(final_status, TaskStatus::Failed(_)) {
            break;
        }
        if final_status == TaskStatus::Cancelled {
            break;
        }
        if let TaskStatus::Failed(message) = &final_status {
            if cancellation.is_cancelled() {
                let cancelled = JournalEntry {
                    id: task.job.id,
                    label: task.job.label.clone(),
                    attempt,
                    status: TaskStatus::Cancelled,
                };
                if let Err(err) = journal.append(&cancelled) {
                    final_status = TaskStatus::Failed(err.to_string());
                    break;
                }
                final_status = TaskStatus::Cancelled;
                break;
            }
            let decision = retry_policy.retry_decision(attempt, message);
            if !decision.retryable {
                break;
            }
            if decision.next_delay_ms > 0
                && attempt < attempts
                && !sleep_retry_backoff_cancellable(decision.next_delay_ms, &cancellation)
            {
                let cancelled = JournalEntry {
                    id: task.job.id,
                    label: task.job.label.clone(),
                    attempt,
                    status: TaskStatus::Cancelled,
                };
                if let Err(err) = journal.append(&cancelled) {
                    final_status = TaskStatus::Failed(err.to_string());
                    break;
                }
                final_status = TaskStatus::Cancelled;
                break;
            }
        }
    }
    final_status
}

fn sleep_retry_backoff_cancellable(delay_ms: u64, cancellation: &Cancellation) -> bool {
    let mut remaining = delay_ms;
    while remaining > 0 {
        if cancellation.is_cancelled() {
            return false;
        }
        let chunk = remaining.min(5);
        thread::sleep(Duration::from_millis(chunk));
        remaining -= chunk;
    }
    !cancellation.is_cancelled()
}

impl Default for WorkerPool {
    fn default() -> Self {
        Self::new(1)
    }
}

#[derive(Debug, Clone)]
struct QueuedJob(Job);

impl PartialEq for QueuedJob {
    fn eq(&self, other: &Self) -> bool {
        self.0.id == other.0.id
    }
}

impl Eq for QueuedJob {}

impl PartialOrd for QueuedJob {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for QueuedJob {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .priority
            .cmp(&other.0.priority)
            .then_with(|| other.0.id.value().cmp(&self.0.id.value()))
    }
}

fn encode_status(status: &TaskStatus) -> String {
    match status {
        TaskStatus::Started => "started".to_string(),
        TaskStatus::Completed => "completed".to_string(),
        TaskStatus::Cancelled => "cancelled".to_string(),
        TaskStatus::Failed(message) => format!("failed:{}", escape(message)),
    }
}

fn parse_status(value: &str) -> std::result::Result<TaskStatus, String> {
    match value {
        "started" => Ok(TaskStatus::Started),
        "completed" => Ok(TaskStatus::Completed),
        "cancelled" => Ok(TaskStatus::Cancelled),
        failed if failed.starts_with("failed:") => Ok(TaskStatus::Failed(unescape(&failed[7..])?)),
        other => Err(format!("invalid task status `{other}`")),
    }
}

fn parse_journal_entry(line: &str) -> std::result::Result<JournalEntry, String> {
    let parts: Vec<_> = line.split('\t').collect();
    if parts.len() != 4 {
        return Err(format!("expected 4 fields, got {}", parts.len()));
    }
    let id = parts[0]
        .parse()
        .map_err(|err| format!("invalid job id `{}`: {err}", parts[0]))?;
    let attempt = parts[1]
        .parse()
        .map_err(|err| format!("invalid attempt `{}`: {err}", parts[1]))?;
    Ok(JournalEntry {
        id: JobId::from_raw(id),
        attempt,
        status: parse_status(parts[2])?,
        label: unescape(parts[3])?,
    })
}

fn parse_payload_record(line: &str) -> std::result::Result<JobPayloadRecord, String> {
    let parts: Vec<_> = line.split('\t').collect();
    if parts.len() != 7 {
        return Err(format!("expected 7 fields, got {}", parts.len()));
    }
    if parts[0] != "payload" {
        return Err(format!("expected payload row, got `{}`", parts[0]));
    }
    let id = parts[1]
        .parse()
        .map_err(|err| format!("invalid payload job id `{}`: {err}", parts[1]))?;
    let kind = JobPayloadKind::parse(parts[2])
        .ok_or_else(|| format!("invalid payload kind `{}`", parts[2]))?;
    let label = unescape(parts[3])?;
    let payload_path = PathBuf::from(unescape(parts[4])?);
    let volume = if parts[5] == "-" {
        None
    } else {
        Some(VolumeId(parts[5].parse().map_err(|err| {
            format!("invalid payload volume id `{}`: {err}", parts[5])
        })?))
    };
    let summary = unescape(parts[6])?;
    Ok(JobPayloadRecord {
        id: JobId::from_raw(id),
        kind,
        label,
        payload_path,
        volume,
        summary,
    })
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

fn real_parent_or_cwd(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

#[cfg(test)]
mod tests;
