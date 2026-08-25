use gfm_types::{GfmError, Result, VolumeId};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::thread;

mod isolated;
use isolated::{IsolatedRetriableTaskQueue, IsolatedTaskQueue};

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

#[derive(Debug, Clone)]
pub struct Job {
    pub id: JobId,
    pub priority: Priority,
    pub label: String,
    pub volume: Option<VolumeId>,
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

#[derive(Debug, Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
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

#[derive(Debug, Default)]
pub struct Scheduler {
    next: AtomicU64,
    queue: BinaryHeap<QueuedJob>,
    cancelled: HashSet<JobId>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn schedule(&mut self, priority: Priority, label: impl Into<String>) -> Job {
        self.schedule_with_volume(priority, label, None)
    }

    pub fn schedule_on_volume(
        &mut self,
        priority: Priority,
        label: impl Into<String>,
        volume: VolumeId,
    ) -> Job {
        self.schedule_with_volume(priority, label, Some(volume))
    }

    fn schedule_with_volume(
        &mut self,
        priority: Priority,
        label: impl Into<String>,
        volume: Option<VolumeId>,
    ) -> Job {
        let id = JobId(self.next.fetch_add(1, AtomicOrdering::SeqCst) + 1);
        let job = Job {
            id,
            priority,
            label: label.into(),
            volume,
            cancel: Cancellation::default(),
        };
        self.queue.push(QueuedJob(job.clone()));
        job
    }

    pub fn cancel(&mut self, id: JobId) {
        self.cancelled.insert(id);
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
pub struct VolumeConcurrencyPolicy {
    default_limit: usize,
    overrides: HashMap<VolumeId, usize>,
}

impl VolumeConcurrencyPolicy {
    pub fn new(default_limit: usize) -> Self {
        Self {
            default_limit: default_limit.max(1),
            overrides: HashMap::new(),
        }
    }

    pub fn with_volume_limit(mut self, volume: VolumeId, limit: usize) -> Self {
        self.overrides.insert(volume, limit.max(1));
        self
    }

    fn limit_for(&self, volume: VolumeId) -> usize {
        self.overrides
            .get(&volume)
            .copied()
            .unwrap_or(self.default_limit)
    }
}

impl Default for VolumeConcurrencyPolicy {
    fn default() -> Self {
        Self::new(1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_attempts: usize,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self { max_attempts: 1 }
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryReason {
    Interrupted,
    RetryableFailure,
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
        if !self.path.exists() {
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
                });
            } else if matches!(state.last_status, Some(TaskStatus::Failed(_)))
                && state.attempts < max_attempts
            {
                jobs.push(RecoveryJob {
                    id: state.id,
                    label: state.label,
                    attempts: state.attempts,
                    reason: RecoveryReason::RetryableFailure,
                });
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
                    let result = task.work.take().expect("worker task missing work")(cancellation);
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
                    let result =
                        lease.task.work.take().expect("worker task missing work")(cancellation);
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
        let attempts = policy.max_attempts.max(1);

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

                    let final_status = execute_retriable_task(&task, &journal, attempts);

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
        let attempts = retry_policy.max_attempts.max(1);

        thread::scope(|scope| {
            for _ in 0..threads {
                let queue = Arc::clone(&queue);
                let outcomes = Arc::clone(&outcomes);
                let journal = journal.clone();
                scope.spawn(move || loop {
                    let Some(lease) = queue.next() else {
                        break;
                    };
                    let final_status = execute_retriable_task(&lease.task, &journal, attempts);
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
    attempts: usize,
) -> TaskStatus {
    let mut final_status = TaskStatus::Failed("task did not run".to_string());
    for attempt in 1..=attempts {
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

        let status = match (task.work)(task.job.cancellation()) {
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
    }
    final_status
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

#[cfg(test)]
mod tests;
