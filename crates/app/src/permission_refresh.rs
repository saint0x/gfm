use crate::runtime::default_permission_state_path;
use gfm_mac::{
    current_permission_onboarding, AccessIntent, MountState, PermissionStateInvalidationReport,
    PermissionStateSnapshot, SecurityDecisionAction, SecurityScopedAccessReport,
    VolumeDiscoveryReport,
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
    Ok(Some(report))
}

pub(crate) fn refresh_permission_state_at_path(
    path: &Path,
) -> Result<PermissionStateInvalidationReport> {
    let probe_path = write_probe_existing_ancestor(path)?;
    let report = VolumeDiscoveryReport::for_containing_path(&probe_path);
    refresh_permission_state_at_path_with_report(path, &probe_path, &report)
}

pub(crate) fn refresh_permission_state_at_path_with_report(
    path: &Path,
    probe_path: &Path,
    report: &VolumeDiscoveryReport,
) -> Result<PermissionStateInvalidationReport> {
    preflight_permission_state_volume_with_report(path, probe_path, report)?;
    let previous = if permission_state_is_file(path)? {
        Some(PermissionStateSnapshot::read(path)?)
    } else {
        None
    };
    let current = PermissionStateSnapshot::from_plan(&current_permission_onboarding()?);
    let report = PermissionStateInvalidationReport::evaluate(previous.as_ref(), &current);
    current.write(path)?;
    Ok(report)
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
    if volume_platform_state_unavailable(volume) {
        return Err(GfmError::Permission {
            path: path.to_path_buf(),
            message: format!(
                "permission state volume access blocked: unavailable volume {}; label={}; root={}; stable-id={}; mount={}; native-status={}; resource-status={}; mount-status={}",
                volume.kind.as_str(),
                volume.label,
                volume.path.display(),
                volume.stable_identity,
                volume.mount_state.as_str(),
                volume
                    .native_status
                    .map(gfm_mac::NativeVolumeStatus::as_str)
                    .unwrap_or("-"),
                volume
                    .resource_status
                    .map(gfm_mac::NativeVolumeStatus::as_str)
                    .unwrap_or("-"),
                volume
                    .mount_table_status
                    .map(gfm_mac::NativeVolumeStatus::as_str)
                    .unwrap_or("-")
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

fn volume_platform_state_unavailable(volume: &gfm_mac::VolumeDescriptor) -> bool {
    volume.native_status == Some(gfm_mac::NativeVolumeStatus::Unavailable)
        && volume.resource_status == Some(gfm_mac::NativeVolumeStatus::Unavailable)
        && volume.mount_table_status == Some(gfm_mac::NativeVolumeStatus::Unavailable)
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

fn permission_state_is_file(path: &Path) -> Result<bool> {
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

fn write_probe_existing_ancestor(path: &Path) -> Result<std::path::PathBuf> {
    let mut candidate = write_probe_path(path)?.to_path_buf();
    while !candidate.try_exists().map_err(|err| {
        GfmError::io(
            &candidate,
            format!("permission state ancestor existence unavailable: {err}"),
        )
    })? {
        let Some(parent) = candidate.parent() else {
            break;
        };
        if parent == candidate {
            break;
        }
        candidate = parent.to_path_buf();
    }
    Ok(candidate)
}

fn escape_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_mac::{
        PermissionReadiness, PermissionScope, PermissionState, VolumeDescriptor, VolumeKind,
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
        volume.resource_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
        volume.mount_table_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
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
        assert!(err.to_string().contains("resource-status=unavailable"));
        assert!(err.to_string().contains("mount-status=unavailable"));
        assert!(!state.exists());
        assert!(!state.parent().unwrap().exists());

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
