use gfm_types::{GfmError, Result, VolumeId};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

mod cancel;
mod isolated;
mod progress;
pub use cancel::Cancellation;
use isolated::{IsolatedRetriableTaskQueue, IsolatedTaskQueue};
pub use progress::{JobProgressSnapshot, JobProgressState, JobProgressStore};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulingPressure {
    pub io: JobIoPressure,
    pub thermal: JobThermalState,
    pub battery: JobBatteryState,
    pub user_activity: JobUserActivity,
}

impl Default for SchedulingPressure {
    fn default() -> Self {
        Self {
            io: JobIoPressure::Nominal,
            thermal: JobThermalState::Nominal,
            battery: JobBatteryState::AcPower,
            user_activity: JobUserActivity::Idle,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobIoPressure {
    Nominal,
    Elevated,
    Saturated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobThermalState {
    Nominal,
    Fair,
    Serious,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobBatteryState {
    AcPower,
    Battery,
    LowPower,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobUserActivity {
    Idle,
    Active,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulingAction {
    Run,
    Throttle,
    Defer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulingDecision {
    pub action: SchedulingAction,
    pub worker_threads: usize,
    pub volume_policy: VolumeConcurrencyPolicy,
}

impl SchedulingPressure {
    pub fn decide(
        self,
        priority: Priority,
        base_threads: usize,
        base_volume_limit: usize,
    ) -> SchedulingDecision {
        let base_threads = base_threads.max(1);
        let base_volume_limit = base_volume_limit.max(1);
        let action = self.action_for(priority);
        let (worker_threads, volume_limit) = match action {
            SchedulingAction::Run => (base_threads, base_volume_limit),
            SchedulingAction::Throttle => (
                throttle_limit(base_threads),
                throttle_limit(base_volume_limit),
            ),
            SchedulingAction::Defer => (0, 1),
        };
        SchedulingDecision {
            action,
            worker_threads,
            volume_policy: VolumeConcurrencyPolicy::new(volume_limit),
        }
    }

    fn action_for(self, priority: Priority) -> SchedulingAction {
        if matches!(priority, Priority::Visible | Priority::Interactive) {
            return SchedulingAction::Run;
        }
        if matches!(self.io, JobIoPressure::Saturated)
            || matches!(self.thermal, JobThermalState::Critical)
        {
            return SchedulingAction::Defer;
        }
        if matches!(self.io, JobIoPressure::Elevated)
            || matches!(self.thermal, JobThermalState::Serious)
            || matches!(self.battery, JobBatteryState::LowPower)
            || matches!(self.user_activity, JobUserActivity::Active)
        {
            return SchedulingAction::Throttle;
        }
        SchedulingAction::Run
    }
}

fn throttle_limit(limit: usize) -> usize {
    (limit / 2).max(1)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobFairnessPolicy {
    quotas: HashMap<JobClass, usize>,
}

impl JobFairnessPolicy {
    pub fn new() -> Self {
        Self {
            quotas: [
                (JobClass::Foreground, 3),
                (JobClass::Visible, 3),
                (JobClass::Background, 1),
                (JobClass::Maintenance, 1),
                (JobClass::Repair, 1),
            ]
            .into_iter()
            .collect(),
        }
    }

    pub fn with_quota(mut self, class: JobClass, quota: usize) -> Self {
        self.quotas.insert(class, quota.max(1));
        self
    }

    fn quota(&self, class: JobClass) -> usize {
        self.quotas.get(&class).copied().unwrap_or(1).max(1)
    }
}

impl Default for JobFairnessPolicy {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedJob {
    pub id: JobId,
    pub label: String,
    pub class: JobClass,
    pub missing_dependencies: Vec<JobId>,
}

#[derive(Debug, Clone)]
pub struct JobFairnessPlan {
    pub ready: Vec<Job>,
    pub blocked: Vec<BlockedJob>,
}

impl JobFairnessPlan {
    pub fn labels(&self) -> Vec<&str> {
        self.ready.iter().map(|job| job.label.as_str()).collect()
    }
}

#[derive(Debug, Clone)]
pub struct JobFairnessPlanner {
    policy: JobFairnessPolicy,
    completed: HashSet<JobId>,
}

impl JobFairnessPlanner {
    pub fn new(policy: JobFairnessPolicy) -> Self {
        Self {
            policy,
            completed: HashSet::new(),
        }
    }

    pub fn with_completed(mut self, completed: impl IntoIterator<Item = JobId>) -> Self {
        self.completed.extend(completed);
        self
    }

    pub fn plan(&self, jobs: impl IntoIterator<Item = Job>) -> JobFairnessPlan {
        let mut pending: VecDeque<Job> = jobs.into_iter().collect();
        let mut satisfied = self.completed.clone();
        let mut ready = Vec::new();

        loop {
            let mut progressed = false;
            for class in JOB_CLASS_ORDER {
                for _ in 0..self.policy.quota(class) {
                    let Some(index) = pending.iter().position(|job| {
                        job.class == class && dependencies_satisfied(job, &satisfied)
                    }) else {
                        break;
                    };
                    let job = pending.remove(index).expect("fairness job vanished");
                    satisfied.insert(job.id);
                    ready.push(job);
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
        }

        let blocked = pending
            .into_iter()
            .map(|job| BlockedJob {
                id: job.id,
                label: job.label,
                class: job.class,
                missing_dependencies: job
                    .dependencies
                    .into_iter()
                    .filter(|dependency| !satisfied.contains(dependency))
                    .collect(),
            })
            .collect();

        JobFairnessPlan { ready, blocked }
    }
}

const JOB_CLASS_ORDER: [JobClass; 5] = [
    JobClass::Foreground,
    JobClass::Visible,
    JobClass::Background,
    JobClass::Maintenance,
    JobClass::Repair,
];

fn dependencies_satisfied(job: &Job, satisfied: &HashSet<JobId>) -> bool {
    job.dependencies
        .iter()
        .all(|dependency| satisfied.contains(dependency))
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

    pub fn default_limit(&self) -> usize {
        self.default_limit
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

impl RetryPolicy {
    pub fn classify_failure(&self, message: &str) -> FailureClass {
        FailureClass::classify(message)
    }

    pub fn retry_decision(&self, attempts: usize, message: &str) -> RetryDecision {
        let class = self.classify_failure(message);
        let max_attempts = self.max_attempts.max(1);
        let retryable = class.retryable() && attempts < max_attempts;
        let next_delay_ms = if retryable {
            retry_backoff_ms(class, attempts)
        } else {
            0
        };
        RetryDecision {
            class,
            retryable,
            next_delay_ms,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    Transient,
    Permission,
    MissingFile,
    CorruptFile,
    OfflineVolume,
    Permanent,
}

impl FailureClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Transient => "transient",
            Self::Permission => "permission",
            Self::MissingFile => "missing-file",
            Self::CorruptFile => "corrupt-file",
            Self::OfflineVolume => "offline-volume",
            Self::Permanent => "permanent",
        }
    }

    pub const fn retryable(self) -> bool {
        matches!(self, Self::Transient | Self::OfflineVolume)
    }

    fn classify(message: &str) -> Self {
        let message = message.to_ascii_lowercase();
        if contains_any(
            &message,
            &["offline", "not mounted", "unmounted", "ejected"],
        ) {
            Self::OfflineVolume
        } else if contains_any(&message, &["permission", "denied", "not permitted", "tcc"]) {
            Self::Permission
        } else if contains_any(&message, &["missing", "not found", "no such file"]) {
            Self::MissingFile
        } else if contains_any(&message, &["corrupt", "checksum", "crc", "malformed"]) {
            Self::CorruptFile
        } else if contains_any(
            &message,
            &[
                "temporary",
                "transient",
                "timed out",
                "timeout",
                "busy",
                "again",
            ],
        ) {
            Self::Transient
        } else {
            Self::Permanent
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryDecision {
    pub class: FailureClass,
    pub retryable: bool,
    pub next_delay_ms: u64,
}

fn retry_backoff_ms(class: FailureClass, attempts: usize) -> u64 {
    let base_ms = match class {
        FailureClass::Transient => 25,
        FailureClass::OfflineVolume => 250,
        FailureClass::Permission
        | FailureClass::MissingFile
        | FailureClass::CorruptFile
        | FailureClass::Permanent => 0,
    };
    if base_ms == 0 {
        return 0;
    }
    let exponent = attempts.saturating_sub(1).min(8) as u32;
    base_ms * 2_u64.pow(exponent)
}

fn contains_any(message: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| message.contains(needle))
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
        let file = File::create(&self.path).map_err(|err| GfmError::io(&self.path, err))?;
        let mut writer = BufWriter::new(file);
        writeln!(writer, "gfm-job-payload-catalog-v1")
            .map_err(|err| GfmError::io(&self.path, err))?;
        for record in records {
            writeln!(writer, "{}", record.as_tsv()).map_err(|err| GfmError::io(&self.path, err))?;
        }
        writer.flush().map_err(|err| GfmError::io(&self.path, err))
    }

    pub fn append(&self, record: &JobPayloadRecord) -> Result<()> {
        if !self.path.exists() {
            self.write_all(&[])?;
        }
        let mut file = OpenOptions::new()
            .append(true)
            .open(&self.path)
            .map_err(|err| GfmError::io(&self.path, err))?;
        writeln!(file, "{}", record.as_tsv()).map_err(|err| GfmError::io(&self.path, err))
    }

    pub fn read(&self) -> Result<Vec<JobPayloadRecord>> {
        if !self.path.exists() {
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
            } else if let Some(TaskStatus::Failed(message)) = &state.last_status {
                if policy.retry_decision(state.attempts, message).retryable
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
        if let TaskStatus::Failed(message) = &final_status {
            let decision = retry_policy.retry_decision(attempt, message);
            if !decision.retryable {
                break;
            }
            if decision.next_delay_ms > 0 && attempt < attempts {
                thread::sleep(Duration::from_millis(decision.next_delay_ms));
            }
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

#[cfg(test)]
mod tests;
