use crate::content::MmapContentArchive;
use crate::contentset::{ContentArchiveManifest, ContentArchiveManifestEntry};
use gfm_types::{GfmError, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentManifestRecoveryAction {
    Ready,
    WriteDiscoveredManifest,
    QuarantineManifestAndWriteDiscovered,
    PruneInvalidArchives,
    CannotRecover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentManifestRecoveryReason {
    Healthy,
    MissingManifest,
    UnreadableManifest,
    MissingArchive,
    UnreadableArchive,
    NoUsableArchives,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentArchiveHealth {
    pub entry: ContentArchiveManifestEntry,
    pub resolved_path: PathBuf,
    pub valid: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentManifestRecoveryPlan {
    pub action: ContentManifestRecoveryAction,
    pub reason: ContentManifestRecoveryReason,
    pub manifest_path: PathBuf,
    pub valid_archives: Vec<ContentArchiveHealth>,
    pub invalid_archives: Vec<ContentArchiveHealth>,
    pub detail: Option<String>,
}

impl ContentManifestRecoveryPlan {
    pub fn ready(&self) -> bool {
        self.action == ContentManifestRecoveryAction::Ready
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "content-manifest-recovery-plan\taction={}\treason={}\tmanifest={}\tvalid={}\tinvalid={}\tdetail={}",
            content_manifest_recovery_action_name(self.action),
            content_manifest_recovery_reason_name(self.reason),
            self.manifest_path.display(),
            self.valid_archives.len(),
            self.invalid_archives.len(),
            self.detail.as_deref().unwrap_or("-")
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentManifestRecovery {
    pub before: ContentManifestRecoveryPlan,
    pub after: ContentManifestRecoveryPlan,
    pub wrote_manifest: bool,
    pub quarantined_manifest_path: Option<PathBuf>,
}

pub fn content_manifest_recovery_action_name(
    action: ContentManifestRecoveryAction,
) -> &'static str {
    match action {
        ContentManifestRecoveryAction::Ready => "ready",
        ContentManifestRecoveryAction::WriteDiscoveredManifest => "write-discovered-manifest",
        ContentManifestRecoveryAction::QuarantineManifestAndWriteDiscovered => {
            "quarantine-manifest-and-write-discovered"
        }
        ContentManifestRecoveryAction::PruneInvalidArchives => "prune-invalid-archives",
        ContentManifestRecoveryAction::CannotRecover => "cannot-recover",
    }
}

pub fn content_manifest_recovery_reason_name(
    reason: ContentManifestRecoveryReason,
) -> &'static str {
    match reason {
        ContentManifestRecoveryReason::Healthy => "healthy",
        ContentManifestRecoveryReason::MissingManifest => "missing-manifest",
        ContentManifestRecoveryReason::UnreadableManifest => "unreadable-manifest",
        ContentManifestRecoveryReason::MissingArchive => "missing-archive",
        ContentManifestRecoveryReason::UnreadableArchive => "unreadable-archive",
        ContentManifestRecoveryReason::NoUsableArchives => "no-usable-archives",
    }
}

pub fn plan_content_manifest_recovery(
    manifest_path: impl AsRef<Path>,
    discovered_archives: &[ContentArchiveManifestEntry],
) -> ContentManifestRecoveryPlan {
    let manifest_path = manifest_path.as_ref().to_path_buf();
    let manifest_exists = match manifest_path.try_exists() {
        Ok(exists) => exists,
        Err(err) => {
            return ContentManifestRecoveryPlan {
                action: ContentManifestRecoveryAction::CannotRecover,
                reason: ContentManifestRecoveryReason::UnreadableManifest,
                manifest_path,
                valid_archives: Vec::new(),
                invalid_archives: Vec::new(),
                detail: Some(format!("content manifest existence unavailable: {err}")),
            }
        }
    };
    if !manifest_exists {
        let (valid, invalid) = classify_archives(&manifest_path, discovered_archives);
        return plan_from_discovered(
            manifest_path,
            valid,
            invalid,
            ContentManifestRecoveryReason::MissingManifest,
            None,
        );
    }

    let manifest = match ContentArchiveManifest::read(&manifest_path) {
        Ok(manifest) => manifest,
        Err(err) => {
            let (valid, invalid) = classify_archives(&manifest_path, discovered_archives);
            return plan_from_discovered(
                manifest_path,
                valid,
                invalid,
                ContentManifestRecoveryReason::UnreadableManifest,
                Some(err.to_string()),
            );
        }
    };

    let (valid, invalid) = classify_archives(&manifest_path, &manifest.archives);
    if invalid.is_empty() {
        return ContentManifestRecoveryPlan {
            action: ContentManifestRecoveryAction::Ready,
            reason: ContentManifestRecoveryReason::Healthy,
            manifest_path,
            valid_archives: valid,
            invalid_archives: invalid,
            detail: None,
        };
    }

    if !valid.is_empty() {
        return ContentManifestRecoveryPlan {
            action: ContentManifestRecoveryAction::PruneInvalidArchives,
            reason: invalid_reason(&invalid),
            manifest_path,
            valid_archives: valid,
            invalid_archives: invalid,
            detail: None,
        };
    }

    let (discovered_valid, discovered_invalid) =
        classify_archives(&manifest_path, discovered_archives);
    if !discovered_valid.is_empty() {
        let mut invalid_archives = invalid;
        invalid_archives.extend(discovered_invalid);
        return ContentManifestRecoveryPlan {
            action: ContentManifestRecoveryAction::WriteDiscoveredManifest,
            reason: invalid_reason(&invalid_archives),
            manifest_path,
            valid_archives: discovered_valid,
            invalid_archives,
            detail: Some("manifest contains no usable active archives".to_string()),
        };
    }

    let mut invalid_archives = invalid;
    invalid_archives.extend(discovered_invalid);
    ContentManifestRecoveryPlan {
        action: ContentManifestRecoveryAction::CannotRecover,
        reason: ContentManifestRecoveryReason::NoUsableArchives,
        manifest_path,
        valid_archives: Vec::new(),
        invalid_archives,
        detail: Some("no valid content archives are available for recovery".to_string()),
    }
}

pub fn recover_content_manifest(
    manifest_path: impl AsRef<Path>,
    discovered_archives: &[ContentArchiveManifestEntry],
    quarantine_dir: impl AsRef<Path>,
) -> Result<ContentManifestRecovery> {
    let manifest_path = manifest_path.as_ref();
    let quarantine_dir = quarantine_dir.as_ref();
    let before = plan_content_manifest_recovery(manifest_path, discovered_archives);
    let mut wrote_manifest = false;
    let mut quarantined_manifest_path = None;

    match before.action {
        ContentManifestRecoveryAction::Ready => {}
        ContentManifestRecoveryAction::WriteDiscoveredManifest
        | ContentManifestRecoveryAction::PruneInvalidArchives => {
            write_recovered_manifest(manifest_path, &before)?;
            wrote_manifest = true;
        }
        ContentManifestRecoveryAction::QuarantineManifestAndWriteDiscovered => {
            let quarantine_path = quarantine_manifest(manifest_path, quarantine_dir)?;
            quarantined_manifest_path = Some(quarantine_path);
            write_recovered_manifest(manifest_path, &before)?;
            wrote_manifest = true;
        }
        ContentManifestRecoveryAction::CannotRecover => {
            return Err(GfmError::Format(format!(
                "{} cannot be recovered: {}",
                manifest_path.display(),
                before
                    .detail
                    .as_deref()
                    .unwrap_or("no usable content archives")
            )))
        }
    }

    let after = plan_content_manifest_recovery(manifest_path, discovered_archives);
    Ok(ContentManifestRecovery {
        before,
        after,
        wrote_manifest,
        quarantined_manifest_path,
    })
}

fn plan_from_discovered(
    manifest_path: PathBuf,
    valid_archives: Vec<ContentArchiveHealth>,
    invalid_archives: Vec<ContentArchiveHealth>,
    reason: ContentManifestRecoveryReason,
    detail: Option<String>,
) -> ContentManifestRecoveryPlan {
    if valid_archives.is_empty() {
        return ContentManifestRecoveryPlan {
            action: ContentManifestRecoveryAction::CannotRecover,
            reason: ContentManifestRecoveryReason::NoUsableArchives,
            manifest_path,
            valid_archives,
            invalid_archives,
            detail: detail
                .or_else(|| Some("no valid discovered content archives are available".to_string())),
        };
    }
    ContentManifestRecoveryPlan {
        action: if reason == ContentManifestRecoveryReason::UnreadableManifest {
            ContentManifestRecoveryAction::QuarantineManifestAndWriteDiscovered
        } else {
            ContentManifestRecoveryAction::WriteDiscoveredManifest
        },
        reason,
        manifest_path,
        valid_archives,
        invalid_archives,
        detail,
    }
}

fn write_recovered_manifest(
    manifest_path: &Path,
    plan: &ContentManifestRecoveryPlan,
) -> Result<()> {
    let entries = plan
        .valid_archives
        .iter()
        .map(|health| health.entry.clone())
        .collect::<Vec<_>>();
    ContentArchiveManifest::new(entries)?.write(manifest_path)
}

fn classify_archives(
    manifest_path: &Path,
    archives: &[ContentArchiveManifestEntry],
) -> (Vec<ContentArchiveHealth>, Vec<ContentArchiveHealth>) {
    let mut valid = Vec::new();
    let mut invalid = Vec::new();
    for entry in archives {
        let resolved_path = resolve_manifest_path(manifest_path, &entry.path);
        let detail = match MmapContentArchive::open(&resolved_path) {
            Ok(_) => None,
            Err(err) => Some(err.to_string()),
        };
        let health = ContentArchiveHealth {
            entry: entry.clone(),
            resolved_path,
            valid: detail.is_none(),
            detail,
        };
        if health.valid {
            valid.push(health);
        } else {
            invalid.push(health);
        }
    }
    (valid, invalid)
}

fn invalid_reason(invalid: &[ContentArchiveHealth]) -> ContentManifestRecoveryReason {
    if invalid
        .iter()
        .any(|health| health.detail.as_deref().is_some_and(is_missing_archive))
    {
        ContentManifestRecoveryReason::MissingArchive
    } else {
        ContentManifestRecoveryReason::UnreadableArchive
    }
}

fn is_missing_archive(detail: &str) -> bool {
    detail.contains("No such file")
        || detail.contains("no such file")
        || detail.contains("os error 2")
        || detail.contains("not found")
}

fn quarantine_manifest(manifest_path: &Path, quarantine_dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(quarantine_dir).map_err(|err| GfmError::io(quarantine_dir, err))?;
    let name = manifest_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("content.gfmmanifest");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let quarantine_path =
        quarantine_dir.join(format!("{name}.corrupt.{}.{}", std::process::id(), nanos));
    fs::rename(manifest_path, &quarantine_path).map_err(|err| GfmError::io(manifest_path, err))?;
    Ok(quarantine_path)
}

fn resolve_manifest_path(manifest_path: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        manifest_path
            .parent()
            .map(|parent| parent.join(path))
            .unwrap_or_else(|| path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{write_content_postings, ContentMergeTier};
    use gfm_types::{ContentPosting, FileId, VolumeId};

    #[test]
    fn content_manifest_recovery_writes_missing_manifest_from_discovered_archives() {
        let dir = temp_dir("gfm-content-recovery-missing");
        let manifest = dir.join("content.gfmmanifest");
        let hot = dir.join("hot.gfmcontent");
        fs::create_dir_all(&dir).unwrap();
        write_content_postings(&hot, &[]).unwrap();

        let discovered = vec![ContentArchiveManifestEntry {
            tier: ContentMergeTier::Hot,
            path: PathBuf::from("hot.gfmcontent"),
        }];
        let plan = plan_content_manifest_recovery(&manifest, &discovered);
        assert_eq!(
            plan.action,
            ContentManifestRecoveryAction::WriteDiscoveredManifest
        );
        assert_eq!(plan.reason, ContentManifestRecoveryReason::MissingManifest);

        let recovery =
            recover_content_manifest(&manifest, &discovered, dir.join("quarantine")).unwrap();

        assert!(recovery.wrote_manifest);
        assert!(recovery.after.ready());
        assert_eq!(
            ContentArchiveManifest::read(&manifest).unwrap().archives,
            discovered
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn content_manifest_recovery_prunes_missing_or_corrupt_archives() {
        let dir = temp_dir("gfm-content-recovery-prune");
        let manifest = dir.join("content.gfmmanifest");
        let good = dir.join("good.gfmcontent");
        let corrupt = dir.join("corrupt.gfmcontent");
        fs::create_dir_all(&dir).unwrap();
        write_content_postings(
            &good,
            &[ContentPosting {
                term: "needle".to_string(),
                ids: vec![FileId::new(VolumeId(1), 1)],
                positions: Vec::new(),
            }],
        )
        .unwrap();
        fs::write(&corrupt, "bad content").unwrap();
        ContentArchiveManifest::new(vec![
            ContentArchiveManifestEntry {
                tier: ContentMergeTier::Hot,
                path: PathBuf::from("good.gfmcontent"),
            },
            ContentArchiveManifestEntry {
                tier: ContentMergeTier::Warm,
                path: PathBuf::from("missing.gfmcontent"),
            },
            ContentArchiveManifestEntry {
                tier: ContentMergeTier::Cold,
                path: PathBuf::from("corrupt.gfmcontent"),
            },
        ])
        .unwrap()
        .write(&manifest)
        .unwrap();

        let plan = plan_content_manifest_recovery(&manifest, &[]);
        assert_eq!(
            plan.action,
            ContentManifestRecoveryAction::PruneInvalidArchives
        );
        assert_eq!(plan.valid_archives.len(), 1);
        assert_eq!(plan.invalid_archives.len(), 2);

        let recovery = recover_content_manifest(&manifest, &[], dir.join("quarantine")).unwrap();

        assert!(recovery.wrote_manifest);
        assert!(recovery.after.ready());
        let recovered = ContentArchiveManifest::read(&manifest).unwrap();
        assert_eq!(
            recovered.archives,
            vec![ContentArchiveManifestEntry {
                tier: ContentMergeTier::Hot,
                path: PathBuf::from("good.gfmcontent"),
            }]
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn content_manifest_recovery_quarantines_unreadable_manifest() {
        let dir = temp_dir("gfm-content-recovery-quarantine");
        let manifest = dir.join("content.gfmmanifest");
        let hot = dir.join("hot.gfmcontent");
        let quarantine = dir.join("quarantine");
        fs::create_dir_all(&dir).unwrap();
        fs::write(&manifest, "not-a-manifest").unwrap();
        write_content_postings(&hot, &[]).unwrap();
        let discovered = vec![ContentArchiveManifestEntry {
            tier: ContentMergeTier::Hot,
            path: PathBuf::from("hot.gfmcontent"),
        }];

        let recovery = recover_content_manifest(&manifest, &discovered, &quarantine).unwrap();

        assert_eq!(
            recovery.before.action,
            ContentManifestRecoveryAction::QuarantineManifestAndWriteDiscovered
        );
        assert!(recovery
            .quarantined_manifest_path
            .as_ref()
            .is_some_and(|path| path.exists()));
        assert!(recovery.after.ready());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn content_manifest_recovery_surfaces_manifest_probe_failures() {
        let dir = temp_dir("gfm-content-recovery-manifest-probe");
        fs::create_dir_all(&dir).unwrap();
        let manifest = dir.join("content-manifest-unavailable".repeat(64));

        let plan = plan_content_manifest_recovery(&manifest, &[]);

        assert_eq!(plan.action, ContentManifestRecoveryAction::CannotRecover);
        assert_eq!(
            plan.reason,
            ContentManifestRecoveryReason::UnreadableManifest
        );
        assert!(plan
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("content manifest existence unavailable")));
        fs::remove_dir_all(dir).unwrap();
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{}-{}",
            prefix,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
}
