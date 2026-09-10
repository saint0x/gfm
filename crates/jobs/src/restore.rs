use crate::{JobId, JobPayloadCatalog, JobPayloadRecord, JobProgressSnapshot, JobProgressStore};
use gfm_types::{GfmError, Result, VolumeId};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobRestoreEntry {
    Ready {
        progress: JobProgressSnapshot,
        payload: JobPayloadRecord,
    },
    MissingPayload {
        progress: JobProgressSnapshot,
    },
}

impl JobRestoreEntry {
    pub const fn progress(&self) -> &JobProgressSnapshot {
        match self {
            Self::Ready { progress, .. } | Self::MissingPayload { progress } => progress,
        }
    }

    pub const fn payload(&self) -> Option<&JobPayloadRecord> {
        match self {
            Self::Ready { payload, .. } => Some(payload),
            Self::MissingPayload { .. } => None,
        }
    }

    pub const fn id(&self) -> JobId {
        self.progress().id
    }

    pub fn as_tsv(&self) -> String {
        match self {
            Self::Ready { progress, payload } => {
                format!("restore\t{}\t{}", progress.state.as_str(), payload.as_tsv())
            }
            Self::MissingPayload { progress } => format!(
                "missing-payload\t{}\t{}\t{}",
                progress.id.value(),
                progress.state.as_str(),
                progress.label
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRestorePlan {
    entries: Vec<JobRestoreEntry>,
}

impl JobRestorePlan {
    pub fn from_catalog_and_progress(
        catalog: &JobPayloadCatalog,
        progress: &JobProgressStore,
        updated_ms: u64,
    ) -> Result<Self> {
        Self::from_catalog_and_progress_checked(catalog, progress, updated_ms, || Ok(()))
    }

    pub fn from_catalog_and_progress_checked(
        catalog: &JobPayloadCatalog,
        progress: &JobProgressStore,
        updated_ms: u64,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check_control()?;
        let restored = progress.restore_interrupted_checked(updated_ms, &mut check_control)?;
        check_control()?;
        let payloads = catalog
            .read_for_ids_checked(
                restored.iter().map(|snapshot| snapshot.id),
                &mut check_control,
            )?
            .into_iter()
            .map(|payload| (payload.id, payload))
            .collect::<HashMap<_, _>>();
        check_control()?;

        let mut entries = Vec::with_capacity(restored.len());
        for snapshot in restored {
            check_control()?;
            match payloads.get(&snapshot.id) {
                Some(payload) => {
                    validate_restore_payload(&snapshot, payload)?;
                    entries.push(JobRestoreEntry::Ready {
                        progress: snapshot,
                        payload: payload.clone(),
                    });
                }
                None => entries.push(JobRestoreEntry::MissingPayload { progress: snapshot }),
            }
            check_control()?;
        }
        Ok(Self { entries })
    }

    pub fn entries(&self) -> &[JobRestoreEntry] {
        &self.entries
    }

    pub fn into_entries(self) -> Vec<JobRestoreEntry> {
        self.entries
    }

    pub fn ready(&self) -> impl Iterator<Item = (&JobProgressSnapshot, &JobPayloadRecord)> {
        self.entries.iter().filter_map(|entry| match entry {
            JobRestoreEntry::Ready { progress, payload } => Some((progress, payload)),
            JobRestoreEntry::MissingPayload { .. } => None,
        })
    }

    pub fn missing_payload(&self) -> impl Iterator<Item = &JobProgressSnapshot> {
        self.entries.iter().filter_map(|entry| match entry {
            JobRestoreEntry::Ready { .. } => None,
            JobRestoreEntry::MissingPayload { progress } => Some(progress),
        })
    }

    pub fn as_tsv_lines(&self) -> Vec<String> {
        self.entries.iter().map(JobRestoreEntry::as_tsv).collect()
    }
}

fn validate_restore_payload(
    progress: &JobProgressSnapshot,
    payload: &JobPayloadRecord,
) -> Result<()> {
    if payload.volume != progress.volume {
        return Err(GfmError::Format(format!(
            "restore payload {} volume {} does not match progress volume {}",
            progress.id.value(),
            format_volume(payload.volume),
            format_volume(progress.volume)
        )));
    }
    Ok(())
}

fn format_volume(volume: Option<VolumeId>) -> String {
    volume
        .map(|volume| volume.0.to_string())
        .unwrap_or_else(|| "-".to_string())
}
