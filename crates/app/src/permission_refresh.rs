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
    let probe_path = write_probe_path(path);
    let report = VolumeDiscoveryReport::for_containing_path(probe_path);
    let Some(volume) = report.volume_for_path(probe_path) else {
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
    path.parent().unwrap_or(path)
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
    use std::path::PathBuf;

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
}
