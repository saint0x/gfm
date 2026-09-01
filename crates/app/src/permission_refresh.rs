use crate::{access::volume_api_status_context, runtime::default_permission_state_path};
use gfm_mac::{
    current_permission_onboarding_checked, AccessIntent, MountState, PermissionScopeChange,
    PermissionStateInvalidationReport, PermissionStateSnapshot, SecurityDecisionAction,
    SecurityScopedAccessReport, VolumeDiscoveryReport,
};
use gfm_types::{GfmError, Result};
use std::fs;
use std::io;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PermissionRefreshAudience {
    Ui,
    Workers,
    Operations,
}

impl PermissionRefreshAudience {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Ui => "ui",
            Self::Workers => "workers",
            Self::Operations => "operations",
        }
    }

    const fn selected(self, report: &PermissionStateInvalidationReport) -> bool {
        match self {
            Self::Ui => report.refresh_ui,
            Self::Workers => report.refresh_workers,
            Self::Operations => report.refresh_operations,
        }
    }
}

pub(crate) fn refresh_permission_state(
    audience: PermissionRefreshAudience,
    subject: &str,
) -> Result<Option<PermissionStateInvalidationReport>> {
    let explicit_state_path = std::env::var_os("GFM_PERMISSION_STATE").is_some();
    if !explicit_state_path && !matches!(audience, PermissionRefreshAudience::Ui) {
        return Ok(None);
    }
    let path = default_permission_state_path();
    let report = refresh_permission_state_at_path(&path)?;
    if !explicit_state_path || report.initialized || !audience.selected(&report) {
        return Ok(None);
    }
    eprintln!(
        "permission-refresh\taudience={}\tsubject={}\tinitialized={}\tchanged={}\trefresh-ui={}\trefresh-workers={}\trefresh-operations={}",
        audience.as_str(),
        escape_field(subject),
        report.initialized,
        report.changed.len(),
        report.refresh_ui,
        report.refresh_workers,
        report.refresh_operations
    );
    for line in permission_refresh_change_lines(audience, subject, &report.changed) {
        eprintln!("{line}");
    }
    Ok(Some(report))
}

pub(crate) fn refresh_permission_state_at_path(
    path: &Path,
) -> Result<PermissionStateInvalidationReport> {
    refresh_permission_state_at_path_checked(path, || Ok(()))
}

pub(crate) fn refresh_permission_state_at_path_checked(
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<PermissionStateInvalidationReport> {
    check_control()?;
    let probe_path = write_probe_existing_ancestor_checked(path, &mut check_control)?;
    check_control()?;
    let report =
        VolumeDiscoveryReport::for_containing_path_checked(&probe_path, &mut check_control)?;
    check_control()?;
    refresh_permission_state_at_path_with_report_checked(path, &probe_path, &report, check_control)
}

pub(crate) fn refresh_permission_state_at_path_with_report(
    path: &Path,
    probe_path: &Path,
    report: &VolumeDiscoveryReport,
) -> Result<PermissionStateInvalidationReport> {
    refresh_permission_state_at_path_with_report_checked(path, probe_path, report, || Ok(()))
}

pub(crate) fn refresh_permission_state_at_path_with_report_checked(
    path: &Path,
    probe_path: &Path,
    report: &VolumeDiscoveryReport,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<PermissionStateInvalidationReport> {
    check_control()?;
    preflight_permission_state_volume_with_report(path, probe_path, report)?;
    check_control()?;
    let previous = if permission_state_is_file_checked(path, &mut check_control)? {
        Some(PermissionStateSnapshot::read_checked(
            path,
            &mut check_control,
        )?)
    } else {
        None
    };
    check_control()?;
    let current = PermissionStateSnapshot::from_plan(&current_permission_onboarding_checked(
        &mut check_control,
    )?);
    let report = PermissionStateInvalidationReport::evaluate(previous.as_ref(), &current);
    check_control()?;
    publish_permission_state_snapshot_checked(path, previous.as_ref(), &current, check_control)?;
    Ok(report)
}

fn permission_state_snapshot_changed(
    previous: Option<&PermissionStateSnapshot>,
    current: &PermissionStateSnapshot,
) -> bool {
    previous != Some(current)
}

fn publish_permission_state_snapshot_checked(
    path: &Path,
    previous: Option<&PermissionStateSnapshot>,
    current: &PermissionStateSnapshot,
    check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    if permission_state_snapshot_changed(previous, current) {
        current.write_checked(path, check_control)?;
    }
    Ok(())
}

fn preflight_permission_state_volume_with_report(
    path: &Path,
    probe_path: &Path,
    report: &VolumeDiscoveryReport,
) -> Result<()> {
    let Some(volume) = report.volume_for_path(probe_path) else {
        return Ok(());
    };
    if volume.mount_state != MountState::Mounted {
        return Err(GfmError::Permission {
            path: path.to_path_buf(),
            message: format!(
                "permission state volume access blocked: unmounted volume {}; label={}; root={}; stable-id={}; mount={}",
                volume.kind.as_str(),
                volume.label,
                volume.path.display(),
                volume.stable_identity,
                volume.mount_state.as_str()
            ),
        });
    }
    if volume.platform_state_unavailable() {
        return Err(GfmError::Permission {
            path: path.to_path_buf(),
            message: format!(
                "permission state volume access blocked: unavailable volume {}; label={}; root={}; stable-id={}; mount={}; {}",
                volume.kind.as_str(),
                volume.label,
                volume.path.display(),
                volume.stable_identity,
                volume.mount_state.as_str(),
                volume_api_status_context(volume)
            ),
        });
    }
    if volume.reachable != Some(false) {
        if !volume.read_only || read_only_root_allows_permission_state_write(volume, probe_path) {
            return Ok(());
        }
        return Err(GfmError::Permission {
            path: path.to_path_buf(),
            message: format!(
                "permission state volume access blocked: read-only volume {}; label={}; root={}; stable-id={}; mount={}",
                volume.kind.as_str(),
                volume.label,
                volume.path.display(),
                volume.stable_identity,
                volume.mount_state.as_str()
            ),
        });
    }
    Err(GfmError::Permission {
        path: path.to_path_buf(),
        message: format!(
            "permission state volume access blocked: unreachable volume {}; label={}; root={}; stable-id={}; mount={}",
            volume.kind.as_str(),
            volume.label,
            volume.path.display(),
            volume.stable_identity,
            volume.mount_state.as_str()
        ),
    })
}

fn read_only_root_allows_permission_state_write(
    volume: &gfm_mac::VolumeDescriptor,
    probe_path: &Path,
) -> bool {
    volume.path == Path::new("/")
        && matches!(
            SecurityScopedAccessReport::evaluate(probe_path, AccessIntent::Write).action,
            SecurityDecisionAction::Allow
        )
}

fn permission_state_is_file_checked(
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<bool> {
    check_control()?;
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(GfmError::io(
            path,
            format!("permission state path metadata unavailable: {err}"),
        )),
    }
}

fn write_probe_path(path: &Path) -> Result<&Path> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(path),
        Ok(_) => Ok(crate::parent_or_cwd(path)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(crate::parent_or_cwd(path)),
        Err(err) => Err(GfmError::io(
            path,
            format!("permission state write probe unavailable: {err}"),
        )),
    }
}

fn write_probe_existing_ancestor_checked(
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<std::path::PathBuf> {
    check_control()?;
    preflight_permission_state_write_target_volume_checked(path, &mut check_control)?;
    check_control()?;
    let mut candidate = write_probe_path(path)?.to_path_buf();
    while !candidate.try_exists().map_err(|err| {
        GfmError::io(
            &candidate,
            format!("permission state ancestor existence unavailable: {err}"),
        )
    })? {
        check_control()?;
        let Some(parent) = candidate.parent() else {
            break;
        };
        if parent == candidate {
            break;
        }
        candidate = parent.to_path_buf();
        check_control()?;
    }
    check_control()?;
    Ok(candidate)
}

fn preflight_permission_state_write_target_volume_checked(
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    check_control()?;
    let volume_path = crate::parent_or_cwd(path);
    let report =
        VolumeDiscoveryReport::for_containing_path_checked(volume_path, &mut check_control)?;
    check_control()?;
    preflight_permission_state_volume_with_report(path, volume_path, &report)
}

fn escape_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

fn permission_refresh_change_lines(
    audience: PermissionRefreshAudience,
    subject: &str,
    changed: &[PermissionScopeChange],
) -> Vec<String> {
    changed
        .iter()
        .map(|change| {
            format!(
                "permission-refresh-change\taudience={}\tsubject={}\tscope={}\tkind={}\tprevious={}\tcurrent={}\tpath={}\treason={}",
                audience.as_str(),
                escape_field(subject),
                change.scope.as_str(),
                change.kind.as_str(),
                change.previous.map(|state| state.as_str()).unwrap_or("-"),
                change.current.as_str(),
                escape_field(&change.path.to_string_lossy()),
                escape_field(&change.reason)
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_mac::{
        PermissionReadiness, PermissionScope, PermissionScopeChange, PermissionScopeChangeKind,
        PermissionState, VolumeDescriptor, VolumeKind,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn audience_selection_tracks_report_refresh_flags() {
        let current = PermissionStateSnapshot {
            readiness: vec![PermissionReadiness {
                scope: PermissionScope::Desktop,
                path: PathBuf::from("/Users/me/Desktop"),
                state: PermissionState::Denied,
                reason: "macOS denied read access".to_string(),
            }],
        };
        let report = PermissionStateInvalidationReport::evaluate(None, &current);

        assert!(PermissionRefreshAudience::Workers.selected(&report));
        assert!(PermissionRefreshAudience::Operations.selected(&report));
    }

    #[test]
    fn permission_refresh_change_lines_preserve_scope_evidence_for_worker_streams() {
        let lines = permission_refresh_change_lines(
            PermissionRefreshAudience::Workers,
            "index\trecords",
            &[PermissionScopeChange {
                scope: PermissionScope::Documents,
                kind: PermissionScopeChangeKind::Revoked,
                path: PathBuf::from("/Users/me/Documents/reports\t2026"),
                previous: Some(PermissionState::Granted),
                current: PermissionState::Denied,
                reason: "macOS denied\nread access".to_string(),
            }],
        );

        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0],
            "permission-refresh-change\taudience=workers\tsubject=index\\trecords\tscope=documents\tkind=revoked\tprevious=granted\tcurrent=denied\tpath=/Users/me/Documents/reports\\t2026\treason=macOS denied\\nread access"
        );
    }

    #[test]
    fn refresh_state_refuses_nested_unreachable_volume_before_parent_creation() {
        let root = unique_temp_dir("gfm-permission-refresh-nested-offline");
        fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
        let state = root.join("runtime").join("permission-state.tsv");

        let err = refresh_permission_state_at_path(&state).unwrap_err();

        assert!(matches!(err, GfmError::Permission { .. }));
        assert!(err
            .to_string()
            .contains("permission state volume access blocked"));
        assert!(!state.exists());
        assert!(!state.parent().unwrap().exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refresh_state_refuses_unreachable_long_state_before_write_probe() {
        let root = unique_temp_dir("gfm-permission-refresh-long-offline");
        fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
        let state = root.join("permission-state-unavailable".repeat(16));

        let err = refresh_permission_state_at_path(&state).unwrap_err();

        assert!(matches!(err, GfmError::Permission { .. }));
        assert!(err
            .to_string()
            .contains("permission state volume access blocked: unreachable volume network"));
        assert!(
            !err.to_string()
                .contains("permission state write probe unavailable"),
            "{err}"
        );
        assert!(!state.exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refresh_state_refuses_nested_read_only_volume_before_parent_creation() {
        let root = unique_temp_dir("gfm-permission-refresh-nested-read-only");
        fs::write(
            root.join(".gfm-volume-kind"),
            "external-removable-read-only\n",
        )
        .unwrap();
        let state = root.join("runtime").join("permission-state.tsv");

        let err = refresh_permission_state_at_path(&state).unwrap_err();

        assert!(matches!(err, GfmError::Permission { .. }));
        assert!(err
            .to_string()
            .contains("permission state volume access blocked: read-only volume external"));
        assert!(!state.exists());
        assert!(!state.parent().unwrap().exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refresh_state_refuses_stale_volume_before_state_write() {
        let root = unique_temp_dir("gfm-permission-refresh-stale-volume");
        let state = root.join("runtime").join("permission-state.tsv");
        let mut volume = VolumeDescriptor::for_path(&root).unwrap();
        volume.kind = VolumeKind::Network;
        volume.mount_state = MountState::Stale;
        volume.reachable = Some(true);
        let report = VolumeDiscoveryReport {
            volumes: vec![volume],
        };

        let err =
            preflight_permission_state_volume_with_report(&state, &root, &report).unwrap_err();

        assert!(matches!(err, GfmError::Permission { .. }));
        assert!(err
            .to_string()
            .contains("permission state volume access blocked: unmounted volume network"));
        assert!(err.to_string().contains("mount=stale"));
        assert!(!state.exists());
        assert!(!state.parent().unwrap().exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refresh_state_refuses_unavailable_volume_api_state_before_write() {
        let root = unique_temp_dir("gfm-permission-refresh-unavailable-api");
        let state = root.join("runtime").join("permission-state.tsv");
        let mut volume = VolumeDescriptor::for_path(&root).unwrap();
        volume.kind = VolumeKind::Network;
        volume.mount_state = MountState::Mounted;
        volume.reachable = Some(true);
        volume.native_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
        volume.native_reason = Some("DiskArbitration unavailable during refresh".to_string());
        volume.resource_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
        volume.resource_reason = Some("URL resource values unavailable during refresh".to_string());
        volume.mount_table_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
        volume.mount_table_reason = Some("mount table unavailable during refresh".to_string());
        let report = VolumeDiscoveryReport {
            volumes: vec![volume],
        };

        let err =
            preflight_permission_state_volume_with_report(&state, &root, &report).unwrap_err();

        assert!(matches!(err, GfmError::Permission { .. }));
        assert!(err
            .to_string()
            .contains("permission state volume access blocked: unavailable volume network"));
        assert!(err.to_string().contains("native-status=unavailable"));
        assert!(err
            .to_string()
            .contains("native-reason=DiskArbitration unavailable during refresh"));
        assert!(err.to_string().contains("resource-status=unavailable"));
        assert!(err
            .to_string()
            .contains("resource-reason=URL resource values unavailable during refresh"));
        assert!(err.to_string().contains("mount-status=unavailable"));
        assert!(err
            .to_string()
            .contains("mount-reason=mount table unavailable during refresh"));
        assert!(!state.exists());
        assert!(!state.parent().unwrap().exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refresh_state_preserves_existing_state_when_cancelled_before_publish() {
        let root = unique_temp_dir("gfm-permission-refresh-publish-cancel");
        let state = root.join("permission-state.tsv");
        let previous = PermissionStateSnapshot {
            readiness: vec![PermissionReadiness {
                scope: PermissionScope::Documents,
                path: root.join("Documents"),
                state: PermissionState::Denied,
                reason: "old denial".to_string(),
            }],
        };
        previous.write(&state).unwrap();
        let before = fs::read(&state).unwrap();
        let mut volume = VolumeDescriptor::for_path(&root).unwrap();
        volume.label = "Local".to_string();
        volume.kind = VolumeKind::Internal;
        volume.mount_state = MountState::Mounted;
        volume.reachable = Some(true);
        volume.read_only = false;
        let report = VolumeDiscoveryReport {
            volumes: vec![volume],
        };
        let mut checks = 0usize;

        let err =
            refresh_permission_state_at_path_with_report_checked(&state, &root, &report, || {
                checks += 1;
                if checks >= 9 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            })
            .unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
        assert!(checks >= 9);
        assert_eq!(fs::read(&state).unwrap(), before);
        let leftovers = fs::read_dir(&root)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".permission-state.tsv")
            })
            .count();
        assert_eq!(leftovers, 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn permission_state_snapshot_publish_gate_accepts_only_real_changes() {
        let previous = PermissionStateSnapshot {
            readiness: vec![PermissionReadiness {
                scope: PermissionScope::Documents,
                path: PathBuf::from("/Users/me/Documents"),
                state: PermissionState::Granted,
                reason: "readable".to_string(),
            }],
        };
        let mut changed = previous.clone();
        changed.readiness[0].state = PermissionState::Denied;
        changed.readiness[0].reason = "macOS denied read access".to_string();

        assert!(permission_state_snapshot_changed(None, &previous));
        assert!(!permission_state_snapshot_changed(
            Some(&previous),
            &previous
        ));
        assert!(permission_state_snapshot_changed(Some(&previous), &changed));
    }

    #[test]
    fn unchanged_permission_state_snapshot_publish_skips_checked_write() {
        let root = unique_temp_dir("gfm-permission-refresh-noop-publish");
        let state = root.join("permission-state.tsv");
        let previous = PermissionStateSnapshot {
            readiness: vec![PermissionReadiness {
                scope: PermissionScope::Documents,
                path: root.join("Documents"),
                state: PermissionState::Granted,
                reason: "readable".to_string(),
            }],
        };
        previous.write(&state).unwrap();
        let before = fs::read(&state).unwrap();

        publish_permission_state_snapshot_checked(&state, Some(&previous), &previous, || {
            Err(GfmError::Cancelled)
        })
        .expect("unchanged permission snapshot should skip checked write entirely");

        assert_eq!(fs::read(&state).unwrap(), before);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn changed_permission_state_snapshot_publish_honors_checked_write_cancellation() {
        let root = unique_temp_dir("gfm-permission-refresh-changed-publish-cancel");
        let state = root.join("permission-state.tsv");
        let previous = PermissionStateSnapshot {
            readiness: vec![PermissionReadiness {
                scope: PermissionScope::Documents,
                path: root.join("Documents"),
                state: PermissionState::Granted,
                reason: "readable".to_string(),
            }],
        };
        let mut changed = previous.clone();
        changed.readiness[0].state = PermissionState::Denied;
        changed.readiness[0].reason = "macOS denied read access".to_string();
        previous.write(&state).unwrap();
        let before = fs::read(&state).unwrap();

        let err =
            publish_permission_state_snapshot_checked(&state, Some(&previous), &changed, || {
                Err(GfmError::Cancelled)
            })
            .unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
        assert_eq!(fs::read(&state).unwrap(), before);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refresh_state_honors_pre_cancelled_control_before_ancestor_probe() {
        let state = std::env::temp_dir()
            .join(format!(
                "gfm-permission-refresh-pre-cancel-{}",
                std::process::id()
            ))
            .join("runtime")
            .join("permission-state.tsv");

        let err = refresh_permission_state_at_path_checked(&state, || Err(GfmError::Cancelled))
            .unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
        assert!(!state.exists());
        assert!(!state.parent().unwrap().exists());
    }

    #[test]
    fn refresh_state_honors_cancellation_before_volume_discovery_finishes() {
        let root = unique_temp_dir("gfm-permission-refresh-volume-discovery-cancel");
        let state = root.join("permission-state.tsv");
        let mut checks = 0usize;

        let err = refresh_permission_state_at_path_checked(&state, || {
            checks += 1;
            if checks >= 5 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        })
        .unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
        assert!(checks >= 5);
        assert!(!state.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refresh_state_preserves_snapshot_when_cancelled_during_current_onboarding() {
        let root = unique_temp_dir("gfm-permission-refresh-onboarding-cancel");
        let state = root.join("permission-state.tsv");
        let snapshot = PermissionStateSnapshot {
            readiness: vec![PermissionReadiness {
                scope: PermissionScope::Documents,
                path: root.join("Documents"),
                state: PermissionState::Granted,
                reason: "previous readable snapshot".to_string(),
            }],
        };
        snapshot.write(&state).unwrap();
        let before = fs::read(&state).unwrap();
        let report = VolumeDiscoveryReport {
            volumes: Vec::new(),
        };
        let mut checks = 0usize;

        let err =
            refresh_permission_state_at_path_with_report_checked(&state, &root, &report, || {
                checks += 1;
                if checks >= 5 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            })
            .unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
        assert_eq!(fs::read(&state).unwrap(), before);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refresh_state_honors_cancellation_before_previous_state_metadata() {
        let root = unique_temp_dir("gfm-permission-refresh-state-metadata-cancel");
        let state = root.join("permission-state.tsv");
        let snapshot = PermissionStateSnapshot {
            readiness: vec![PermissionReadiness {
                scope: PermissionScope::Documents,
                path: root.join("Documents"),
                state: PermissionState::Granted,
                reason: "previous readable snapshot".to_string(),
            }],
        };
        snapshot.write(&state).unwrap();
        let before = fs::read(&state).unwrap();
        let report = VolumeDiscoveryReport {
            volumes: Vec::new(),
        };
        let mut checks = 0usize;

        let err =
            refresh_permission_state_at_path_with_report_checked(&state, &root, &report, || {
                checks += 1;
                if checks >= 2 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            })
            .unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
        assert_eq!(fs::read(&state).unwrap(), before);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn write_probe_existing_ancestor_checked_can_cancel_while_climbing() {
        let root = unique_temp_dir("gfm-permission-refresh-ancestor-cancel");
        let state = root
            .join("missing")
            .join("deep")
            .join("permission-state.tsv");
        let mut checks = 0usize;

        let err = write_probe_existing_ancestor_checked(&state, || {
            checks += 1;
            if checks >= 3 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        })
        .unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
        assert!(checks >= 3);
        assert!(!root.join("missing").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refresh_state_refuses_read_only_system_root_when_probe_is_denied() {
        let state = PathBuf::from("/System/gfm-permission-state.tsv");
        let mut volume = VolumeDescriptor::for_path("/").unwrap();
        volume.read_only = true;
        volume.writable = false;
        let report = VolumeDiscoveryReport {
            volumes: vec![volume],
        };

        let err =
            preflight_permission_state_volume_with_report(&state, Path::new("/System"), &report)
                .unwrap_err();

        assert!(matches!(err, GfmError::Permission { .. }));
        assert!(err
            .to_string()
            .contains("permission state volume access blocked: read-only volume system"));
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "{}-{}-{}",
            prefix,
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }
}
