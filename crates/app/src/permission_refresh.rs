use crate::runtime::default_permission_state_path;
use gfm_mac::{
    current_permission_onboarding, PermissionStateInvalidationReport, PermissionStateSnapshot,
    VolumeDiscoveryReport,
};
use gfm_types::{GfmError, Result};
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
    let path = default_permission_state_path();
    let report = refresh_permission_state_at_path(&path)?;
    let explicit_state_path = std::env::var_os("GFM_PERMISSION_STATE").is_some();
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
    preflight_permission_state_volume(path)?;
    let previous = if path.is_file() {
        Some(PermissionStateSnapshot::read(path)?)
    } else {
        None
    };
    let current = PermissionStateSnapshot::from_plan(&current_permission_onboarding()?);
    let report = PermissionStateInvalidationReport::evaluate(previous.as_ref(), &current);
    current.write(path)?;
    Ok(report)
}

fn preflight_permission_state_volume(path: &Path) -> Result<()> {
    let probe_path = write_probe_existing_ancestor(path);
    let report = VolumeDiscoveryReport::for_containing_path(&probe_path);
    let Some(volume) = report.volume_for_path(&probe_path) else {
        return Ok(());
    };
    if volume.reachable != Some(false) {
        return Ok(());
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

fn write_probe_path(path: &Path) -> &Path {
    if path.is_dir() {
        return path;
    }
    crate::parent_or_cwd(path)
}

fn write_probe_existing_ancestor(path: &Path) -> std::path::PathBuf {
    let mut candidate = write_probe_path(path).to_path_buf();
    while !candidate.exists() {
        let Some(parent) = candidate.parent() else {
            break;
        };
        if parent == candidate {
            break;
        }
        candidate = parent.to_path_buf();
    }
    candidate
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
    use gfm_mac::{PermissionReadiness, PermissionScope, PermissionState};
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
