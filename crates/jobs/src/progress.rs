use crate::{escape, unescape, JobClass, JobId, Priority, TaskStatus};
use gfm_types::{GfmError, Result, VolumeId};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

const MAGIC: &str = "gfm-job-progress-v1";
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn progress_path_exists(path: &Path) -> Result<bool> {
    path.try_exists().map_err(|err| {
        GfmError::io(
            path,
            format!("job progress store existence unavailable: {err}"),
        )
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobProgressState {
    Planned,
    Running,
    Paused,
    Completed,
    Cancelled,
    Failed,
}

impl JobProgressState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "planned" => Some(Self::Planned),
            "running" => Some(Self::Running),
            "paused" => Some(Self::Paused),
            "completed" => Some(Self::Completed),
            "cancelled" => Some(Self::Cancelled),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }

    pub const fn restorable(self) -> bool {
        matches!(self, Self::Planned | Self::Running | Self::Paused)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobProgressCommand {
    Pause,
    Resume,
    Stop,
}

impl JobProgressCommand {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Stop => "stop",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pause" => Some(Self::Pause),
            "resume" => Some(Self::Resume),
            "stop" => Some(Self::Stop),
            _ => None,
        }
    }
}

impl From<&TaskStatus> for JobProgressState {
    fn from(status: &TaskStatus) -> Self {
        match status {
            TaskStatus::Started => Self::Running,
            TaskStatus::Completed => Self::Completed,
            TaskStatus::Cancelled => Self::Cancelled,
            TaskStatus::Failed(_) => Self::Failed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobProgressSnapshot {
    pub id: JobId,
    pub class: JobClass,
    pub priority: Priority,
    pub label: String,
    pub volume: Option<VolumeId>,
    pub state: JobProgressState,
    pub completed_units: u64,
    pub total_units: u64,
    pub detail: String,
    pub updated_ms: u64,
}

impl JobProgressSnapshot {
    pub fn new(
        id: JobId,
        class: JobClass,
        priority: Priority,
        label: impl Into<String>,
        volume: Option<VolumeId>,
        total_units: u64,
    ) -> Self {
        Self {
            id,
            class,
            priority,
            label: label.into(),
            volume,
            state: JobProgressState::Planned,
            completed_units: 0,
            total_units,
            detail: String::new(),
            updated_ms: 0,
        }
    }

    pub fn with_progress(
        mut self,
        state: JobProgressState,
        completed_units: u64,
        detail: impl Into<String>,
        updated_ms: u64,
    ) -> Self {
        self.state = state;
        self.completed_units = completed_units.min(self.total_units);
        self.detail = detail.into();
        self.updated_ms = updated_ms;
        self
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "progress\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.id.value(),
            self.class.as_str(),
            self.priority.as_str(),
            escape(&self.label),
            self.volume
                .map(|volume| volume.0.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.state.as_str(),
            self.completed_units,
            self.total_units,
            escape(&self.detail),
            self.updated_ms,
        )
    }
}

#[derive(Debug, Clone)]
pub struct JobProgressStore {
    path: PathBuf,
}

impl JobProgressStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write_all(&self, snapshots: &[JobProgressSnapshot]) -> Result<()> {
        let parent = real_parent_or_cwd(&self.path);
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
        let tmp = self.temp_path();
        {
            let file = File::create(&tmp).map_err(|err| GfmError::io(&tmp, err))?;
            let mut writer = BufWriter::new(file);
            writeln!(writer, "{MAGIC}").map_err(|err| GfmError::io(&tmp, err))?;
            for snapshot in snapshots {
                writeln!(writer, "{}", snapshot.as_tsv()).map_err(|err| GfmError::io(&tmp, err))?;
            }
            writer.flush().map_err(|err| GfmError::io(&tmp, err))?;
        }
        fs::rename(&tmp, &self.path).map_err(|err| GfmError::io(&self.path, err))
    }

    pub fn upsert(&self, snapshot: JobProgressSnapshot) -> Result<bool> {
        let mut snapshots = self.read()?;
        if let Some(existing) = snapshots
            .iter_mut()
            .find(|existing| existing.id == snapshot.id)
        {
            if *existing == snapshot {
                return Ok(false);
            }
            *existing = snapshot;
        } else {
            snapshots.push(snapshot);
        }
        snapshots.sort_by_key(|snapshot| snapshot.id.value());
        self.write_all(&snapshots)?;
        Ok(true)
    }

    pub fn read(&self) -> Result<Vec<JobProgressSnapshot>> {
        self.read_filtered(|_| true)
    }

    pub fn restorable(&self) -> Result<Vec<JobProgressSnapshot>> {
        self.read_filtered(|snapshot| snapshot.state.restorable())
    }

    fn read_filtered(
        &self,
        mut keep: impl FnMut(&JobProgressSnapshot) -> bool,
    ) -> Result<Vec<JobProgressSnapshot>> {
        if !progress_path_exists(&self.path)? {
            return Ok(Vec::new());
        }
        let file = File::open(&self.path).map_err(|err| GfmError::io(&self.path, err))?;
        let mut lines = BufReader::new(file).lines();
        let header = lines
            .next()
            .transpose()
            .map_err(|err| GfmError::io(&self.path, err))?
            .ok_or_else(|| {
                GfmError::Format(format!("empty job progress store {}", self.path.display()))
            })?;
        if header != MAGIC {
            return Err(GfmError::Format(format!(
                "unsupported job progress header `{header}` in {}",
                self.path.display()
            )));
        }
        let mut snapshots = Vec::new();
        for (line_index, line) in lines.enumerate() {
            let line = line.map_err(|err| GfmError::io(&self.path, err))?;
            let snapshot = parse_snapshot(&line).map_err(|err| {
                GfmError::Format(format!(
                    "{} line {}: {}",
                    self.path.display(),
                    line_index + 2,
                    err
                ))
            })?;
            if keep(&snapshot) {
                snapshots.push(snapshot);
            }
        }
        Ok(snapshots)
    }

    pub fn restore_interrupted(&self, updated_ms: u64) -> Result<Vec<JobProgressSnapshot>> {
        if !progress_path_exists(&self.path)? {
            return Ok(Vec::new());
        }
        let mut snapshots = self.read()?;
        let mut changed = false;
        for snapshot in &mut snapshots {
            if matches!(
                snapshot.state,
                JobProgressState::Planned | JobProgressState::Running
            ) {
                let previous_state = snapshot.state.as_str();
                snapshot.state = JobProgressState::Paused;
                snapshot.detail = if snapshot.detail.is_empty() {
                    format!("interrupted:{previous_state}")
                } else {
                    format!("interrupted:{previous_state}:{}", snapshot.detail)
                };
                snapshot.updated_ms = updated_ms;
                changed = true;
            }
        }
        if changed {
            self.write_all(&snapshots)?;
        }
        Ok(snapshots
            .into_iter()
            .filter(|snapshot| snapshot.state.restorable())
            .collect())
    }

    pub fn apply_command(
        &self,
        id: JobId,
        command: JobProgressCommand,
        updated_ms: u64,
    ) -> Result<JobProgressSnapshot> {
        let mut snapshots = self.read()?;
        let Some(snapshot) = snapshots.iter_mut().find(|snapshot| snapshot.id == id) else {
            return Err(GfmError::Format(format!(
                "job progress store {} does not contain job {}",
                self.path.display(),
                id.value()
            )));
        };
        if !apply_progress_command(snapshot, command, updated_ms)? {
            return Ok(snapshot.clone());
        }
        let updated = snapshot.clone();
        self.write_all(&snapshots)?;
        Ok(updated)
    }

    fn temp_path(&self) -> PathBuf {
        let mut name = self
            .path
            .file_name()
            .map(|name| name.to_os_string())
            .unwrap_or_else(|| "job-progress".into());
        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed);
        name.push(format!(".{}.{sequence}.tmp", std::process::id()));
        self.path.with_file_name(name)
    }
}

fn apply_progress_command(
    snapshot: &mut JobProgressSnapshot,
    command: JobProgressCommand,
    updated_ms: u64,
) -> Result<bool> {
    match command {
        JobProgressCommand::Pause => {
            if matches!(
                snapshot.state,
                JobProgressState::Completed
                    | JobProgressState::Cancelled
                    | JobProgressState::Failed
            ) {
                return Err(terminal_command_error(snapshot, command));
            }
            if snapshot.state == JobProgressState::Paused
                && snapshot.detail.starts_with("paused-by-user")
            {
                return Ok(false);
            }
            snapshot.state = JobProgressState::Paused;
            snapshot.detail = command_detail("paused-by-user", &snapshot.detail);
        }
        JobProgressCommand::Resume => {
            if snapshot.state != JobProgressState::Paused {
                return Err(GfmError::Format(format!(
                    "cannot resume job {} from {} progress state",
                    snapshot.id.value(),
                    snapshot.state.as_str()
                )));
            }
            snapshot.state = JobProgressState::Running;
            snapshot.detail = command_detail("resumed-by-user", &snapshot.detail);
        }
        JobProgressCommand::Stop => {
            if !snapshot.state.restorable() {
                return Err(terminal_command_error(snapshot, command));
            }
            snapshot.state = JobProgressState::Cancelled;
            snapshot.detail = command_detail("cancelled-by-user", &snapshot.detail);
        }
    }
    snapshot.updated_ms = updated_ms;
    Ok(true)
}

fn command_detail(prefix: &str, previous: &str) -> String {
    if previous.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}:{previous}")
    }
}

fn terminal_command_error(snapshot: &JobProgressSnapshot, command: JobProgressCommand) -> GfmError {
    GfmError::Format(format!(
        "cannot {} job {} from {} progress state",
        command.as_str(),
        snapshot.id.value(),
        snapshot.state.as_str()
    ))
}

fn parse_snapshot(line: &str) -> std::result::Result<JobProgressSnapshot, String> {
    let parts: Vec<_> = line.split('\t').collect();
    if parts.len() != 11 {
        return Err(format!("expected 11 fields, got {}", parts.len()));
    }
    if parts[0] != "progress" {
        return Err(format!("expected progress row, got `{}`", parts[0]));
    }
    let id = parts[1]
        .parse()
        .map_err(|err| format!("invalid progress job id `{}`: {err}", parts[1]))?;
    let class =
        JobClass::parse(parts[2]).ok_or_else(|| format!("invalid job class `{}`", parts[2]))?;
    let priority =
        Priority::parse(parts[3]).ok_or_else(|| format!("invalid priority `{}`", parts[3]))?;
    let label = unescape(parts[4])?;
    let volume = if parts[5] == "-" {
        None
    } else {
        Some(VolumeId(parts[5].parse().map_err(|err| {
            format!("invalid progress volume id `{}`: {err}", parts[5])
        })?))
    };
    let state = JobProgressState::parse(parts[6])
        .ok_or_else(|| format!("invalid progress state `{}`", parts[6]))?;
    let completed_units = parts[7]
        .parse()
        .map_err(|err| format!("invalid completed units `{}`: {err}", parts[7]))?;
    let total_units = parts[8]
        .parse()
        .map_err(|err| format!("invalid total units `{}`: {err}", parts[8]))?;
    let detail = unescape(parts[9])?;
    let updated_ms = parts[10]
        .parse()
        .map_err(|err| format!("invalid updated ms `{}`: {err}", parts[10]))?;
    Ok(JobProgressSnapshot {
        id: JobId::from_raw(id),
        class,
        priority,
        label,
        volume,
        state,
        completed_units,
        total_units,
        detail,
        updated_ms,
    })
}

fn real_parent_or_cwd(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn progress_store_temp_paths_are_unique_within_process() {
        let path = temp_path("unique-temp");
        let store = JobProgressStore::new(&path);

        let first = store.temp_path();
        let second = store.temp_path();

        assert_ne!(first, second);
        assert_eq!(first.parent(), path.parent());
        assert_eq!(second.parent(), path.parent());
        assert!(first
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".tmp")));
        assert!(second
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".tmp")));
    }

    #[test]
    fn restorable_streams_only_active_snapshots_but_validates_terminal_rows() {
        let path = temp_path("restorable-filtered");
        let store = JobProgressStore::new(&path);
        let running = JobProgressSnapshot::new(
            JobId::from_raw(1),
            JobClass::Foreground,
            Priority::Interactive,
            "copy selected files",
            Some(VolumeId(2)),
            10,
        )
        .with_progress(JobProgressState::Running, 4, "copying", 10);
        let completed = JobProgressSnapshot::new(
            JobId::from_raw(2),
            JobClass::Maintenance,
            Priority::Background,
            "compact content",
            None,
            1,
        )
        .with_progress(JobProgressState::Completed, 1, "done", 11);
        store
            .write_all(&[running.clone(), completed.clone()])
            .unwrap();

        assert_eq!(store.restorable().unwrap(), vec![running]);
        fs::write(
            &path,
            format!(
                "{MAGIC}\n{}\nprogress\tbad-id\tmaintenance\tbackground\tcompact content\t-\tcompleted\t1\t1\tdone\t11\n",
                completed.as_tsv()
            ),
        )
        .unwrap();
        let err = store.restorable().unwrap_err();
        assert!(
            err.to_string().contains("invalid progress job id `bad-id`"),
            "{err}"
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn progress_commands_pause_resume_and_stop_persist_atomically() {
        let path = temp_path("progress-commands");
        let store = JobProgressStore::new(&path);
        let snapshot = JobProgressSnapshot::new(
            JobId::from_raw(7),
            JobClass::Foreground,
            Priority::Interactive,
            "copy selected files",
            Some(VolumeId(2)),
            100,
        )
        .with_progress(JobProgressState::Running, 12, "copying", 1);
        store.write_all(&[snapshot]).unwrap();

        let paused = store
            .apply_command(JobId::from_raw(7), JobProgressCommand::Pause, 2)
            .unwrap();
        assert_eq!(paused.state, JobProgressState::Paused);
        assert_eq!(paused.detail, "paused-by-user:copying");

        let resumed = store
            .apply_command(JobId::from_raw(7), JobProgressCommand::Resume, 3)
            .unwrap();
        assert_eq!(resumed.state, JobProgressState::Running);
        assert_eq!(resumed.detail, "resumed-by-user:paused-by-user:copying");

        let stopped = store
            .apply_command(JobId::from_raw(7), JobProgressCommand::Stop, 4)
            .unwrap();
        assert_eq!(stopped.state, JobProgressState::Cancelled);
        assert_eq!(
            stopped.detail,
            "cancelled-by-user:resumed-by-user:paused-by-user:copying"
        );
        assert_eq!(store.restorable().unwrap(), Vec::new());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn repeated_user_pause_does_not_rewrite_progress_store() {
        let path = temp_path("progress-pause-noop");
        let store = JobProgressStore::new(&path);
        let snapshot = JobProgressSnapshot::new(
            JobId::from_raw(7),
            JobClass::Foreground,
            Priority::Interactive,
            "copy selected files",
            Some(VolumeId(2)),
            100,
        )
        .with_progress(JobProgressState::Running, 12, "copying", 1);
        store.write_all(&[snapshot]).unwrap();
        let paused = store
            .apply_command(JobId::from_raw(7), JobProgressCommand::Pause, 2)
            .unwrap();
        let before = fs::metadata(&path).unwrap().modified().unwrap();

        let repeated = store
            .apply_command(JobId::from_raw(7), JobProgressCommand::Pause, 3)
            .unwrap();
        let after = fs::metadata(&path).unwrap().modified().unwrap();

        assert_eq!(repeated, paused);
        assert_eq!(before, after);
        assert_eq!(store.read().unwrap(), vec![paused]);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn progress_commands_reject_invalid_state_transitions() {
        let path = temp_path("progress-command-invalid");
        let store = JobProgressStore::new(&path);
        let completed = JobProgressSnapshot::new(
            JobId::from_raw(9),
            JobClass::Maintenance,
            Priority::Background,
            "compact content",
            None,
            1,
        )
        .with_progress(JobProgressState::Completed, 1, "done", 1);
        store.write_all(&[completed]).unwrap();

        let err = store
            .apply_command(JobId::from_raw(9), JobProgressCommand::Pause, 2)
            .unwrap_err();
        assert!(err.to_string().contains("cannot pause job 9"));

        let _ = fs::remove_file(path);
    }

    fn temp_path(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "gfm-job-progress-{label}-{}-{nanos}.gfmprogress",
            std::process::id()
        ))
    }
}
