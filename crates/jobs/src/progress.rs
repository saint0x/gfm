use crate::{escape, unescape, JobClass, JobId, Priority, TaskStatus};
use gfm_types::{GfmError, Result, VolumeId};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

const MAGIC: &str = "gfm-job-progress-v1";

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
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
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

    pub fn upsert(&self, snapshot: JobProgressSnapshot) -> Result<()> {
        let mut snapshots = self.read()?;
        if let Some(existing) = snapshots
            .iter_mut()
            .find(|existing| existing.id == snapshot.id)
        {
            *existing = snapshot;
        } else {
            snapshots.push(snapshot);
        }
        snapshots.sort_by_key(|snapshot| snapshot.id.value());
        self.write_all(&snapshots)
    }

    pub fn read(&self) -> Result<Vec<JobProgressSnapshot>> {
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
            snapshots.push(parse_snapshot(&line).map_err(|err| {
                GfmError::Format(format!(
                    "{} line {}: {}",
                    self.path.display(),
                    line_index + 2,
                    err
                ))
            })?);
        }
        Ok(snapshots)
    }

    pub fn restorable(&self) -> Result<Vec<JobProgressSnapshot>> {
        Ok(self
            .read()?
            .into_iter()
            .filter(|snapshot| snapshot.state.restorable())
            .collect())
    }

    pub fn restore_interrupted(&self, updated_ms: u64) -> Result<Vec<JobProgressSnapshot>> {
        let mut snapshots = self.read()?;
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
            }
        }
        self.write_all(&snapshots)?;
        Ok(snapshots
            .into_iter()
            .filter(|snapshot| snapshot.state.restorable())
            .collect())
    }

    fn temp_path(&self) -> PathBuf {
        let mut name = self
            .path
            .file_name()
            .map(|name| name.to_os_string())
            .unwrap_or_else(|| "job-progress".into());
        name.push(format!(".{}.tmp", std::process::id()));
        self.path.with_file_name(name)
    }
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
