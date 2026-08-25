use crate::{FseventsResumeAction, FseventsResumePlan, IndexVolumeState};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RepairPriority {
    Normal,
    High,
    Critical,
}

impl RepairPriority {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairReason {
    ResumeRequired(String),
    EventIdGap { expected: u64, observed: u64 },
    ExplicitDrop(String),
}

impl RepairReason {
    pub fn as_str(&self) -> String {
        match self {
            Self::ResumeRequired(reason) => format!("resume-required:{reason}"),
            Self::EventIdGap { expected, observed } => {
                format!("event-id-gap:{expected}-{observed}")
            }
            Self::ExplicitDrop(reason) => format!("explicit-drop:{reason}"),
        }
    }

    pub const fn priority(&self) -> RepairPriority {
        match self {
            Self::ResumeRequired(_) => RepairPriority::Critical,
            Self::EventIdGap { .. } => RepairPriority::High,
            Self::ExplicitDrop(_) => RepairPriority::High,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtreeRepairJob {
    pub path: PathBuf,
    pub reason: RepairReason,
    pub priority: RepairPriority,
}

impl SubtreeRepairJob {
    fn new(path: impl Into<PathBuf>, reason: RepairReason) -> Self {
        let reason_priority = reason.priority();
        Self {
            path: path.into(),
            reason,
            priority: reason_priority,
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "repair\tpath={}\tpriority={}\treason={}",
            self.path.display(),
            self.priority.as_str(),
            self.reason.as_str()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairSchedule {
    pub resume: FseventsResumePlan,
    pub jobs: Vec<SubtreeRepairJob>,
    pub highest_observed_event_id: Option<u64>,
}

impl RepairSchedule {
    pub fn evaluate(
        volume: &IndexVolumeState,
        resume: FseventsResumePlan,
        observed_event_ids: &[u64],
        dropped_roots: &[PathBuf],
        explicit_reason: Option<&str>,
    ) -> Self {
        let mut jobs = Vec::new();
        if resume.action == FseventsResumeAction::Rescan {
            jobs.push(SubtreeRepairJob::new(
                volume.root.clone(),
                RepairReason::ResumeRequired(resume.reason.clone()),
            ));
        }

        let mut expected = resume.from_event_id;
        for observed in observed_event_ids.iter().copied() {
            if let Some(next_expected) = expected {
                if observed > next_expected {
                    jobs.push(SubtreeRepairJob::new(
                        volume.root.clone(),
                        RepairReason::EventIdGap {
                            expected: next_expected,
                            observed,
                        },
                    ));
                }
            }
            expected = Some(observed.saturating_add(1));
        }

        let reason = explicit_reason.unwrap_or("stream-drop");
        for root in dropped_roots {
            jobs.push(SubtreeRepairJob::new(
                normalize_repair_path(&volume.root, root),
                RepairReason::ExplicitDrop(reason.to_string()),
            ));
        }

        Self {
            resume,
            jobs: coalesce_jobs(jobs),
            highest_observed_event_id: observed_event_ids.iter().copied().max(),
        }
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = vec![format!(
            "repair-schedule\taction={}\tfrom-event-id={}\tjobs={}\thighest-observed-event-id={}\treason={}",
            self.resume.action.as_str(),
            self.resume
                .from_event_id
                .map(|event_id| event_id.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.jobs.len(),
            self.highest_observed_event_id
                .map(|event_id| event_id.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.resume.reason
        )];
        for job in &self.jobs {
            lines.push(job.as_tsv());
        }
        lines.join("\n")
    }
}

fn coalesce_jobs(mut jobs: Vec<SubtreeRepairJob>) -> Vec<SubtreeRepairJob> {
    jobs.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| b.priority.cmp(&a.priority))
            .then_with(|| a.reason.as_str().cmp(&b.reason.as_str()))
    });
    let mut coalesced: Vec<SubtreeRepairJob> = Vec::new();
    for job in jobs {
        if coalesced
            .iter()
            .any(|existing| job.path.starts_with(&existing.path))
        {
            continue;
        }
        coalesced.retain(|existing| !existing.path.starts_with(&job.path));
        coalesced.push(job);
    }
    coalesced
}

fn normalize_repair_path(root: &Path, path: &Path) -> PathBuf {
    if path.as_os_str().is_empty() || path == Path::new("-") {
        return root.to_path_buf();
    }
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}
