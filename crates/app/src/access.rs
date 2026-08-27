use crate::{
    permission_refresh::{refresh_permission_state, PermissionRefreshAudience},
    runtime::default_security_bookmarks_path,
};
use gfm_mac::{
    AccessIntent, SecurityDecisionAction, SecurityScopedAccessReport, SecurityScopedBookmarkAccess,
    SecurityScopedBookmarkStore, SecurityWorkerAction, VolumeDiscoveryReport,
};
use gfm_types::{GfmError, Result};
use std::path::Path;

pub(crate) struct ScopedAccessGuard {
    _accesses: Vec<SecurityScopedBookmarkAccess>,
}

pub(crate) fn preflight_access_scope(
    path: &Path,
    intent: AccessIntent,
    worker: &str,
) -> Result<ScopedAccessGuard> {
    let _ = refresh_permission_state(PermissionRefreshAudience::Workers, worker)?;
    preflight_volume_reachability(path, worker)?;
    let report = SecurityScopedAccessReport::evaluate(path, intent);
    eprintln!("{}", report.as_tsv());
    let admission = report.worker_admission(worker);
    eprintln!("{}", admission.as_tsv());
    match admission.worker_action {
        SecurityWorkerAction::Start => {
            let accesses = retained_security_accesses(&report)?;
            Ok(ScopedAccessGuard {
                _accesses: accesses,
            })
        }
        SecurityWorkerAction::MetadataOnly => Err(GfmError::Permission {
            path: path.to_path_buf(),
            message: format!("{worker} access degraded: {}", report.reason),
        }),
        SecurityWorkerAction::Prompt | SecurityWorkerAction::Deny => Err(GfmError::Permission {
            path: path.to_path_buf(),
            message: format!("{worker} access blocked: {}", report.reason),
        }),
    }
}

fn preflight_volume_reachability(path: &Path, worker: &str) -> Result<()> {
    let volume_path = absolute_volume_probe_path(path);
    let report = VolumeDiscoveryReport::for_containing_path(&volume_path);
    preflight_volume_reachability_in_report(path, &volume_path, worker, &report)
}

fn preflight_volume_reachability_in_report(
    user_path: &Path,
    volume_path: &Path,
    worker: &str,
    report: &VolumeDiscoveryReport,
) -> Result<()> {
    let Some(volume) = report.volume_for_path(volume_path) else {
        return Ok(());
    };
    if volume.reachable != Some(false) {
        return Ok(());
    }
    Err(GfmError::Permission {
        path: user_path.to_path_buf(),
        message: format!(
            "{worker} volume access blocked: unreachable volume {}; label={}; root={}; stable-id={}; mount={}",
            volume.kind.as_str(),
            volume.label,
            volume.path.display(),
            volume.stable_identity,
            volume.mount_state.as_str()
        ),
    })
}

fn absolute_volume_probe_path(path: &Path) -> std::path::PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

fn retained_security_accesses(
    report: &SecurityScopedAccessReport,
) -> Result<Vec<SecurityScopedBookmarkAccess>> {
    if !report.bookmark_required || !matches!(report.action, SecurityDecisionAction::Allow) {
        return Ok(Vec::new());
    }
    let store = SecurityScopedBookmarkStore::new(default_security_bookmarks_path());
    let lookup =
        store.start_access_for_path(&report.path, read_only_intent(report.intent), true)?;
    let Some(access) = lookup.access else {
        eprintln!(
            "security-scope-access\t{}\tstatus=missing\tread-only={}\taccess-started=false\treason=current-access-without-retained-bookmark",
            report.path.display(),
            read_only_intent(report.intent)
        );
        return Ok(Vec::new());
    };
    eprintln!(
        "security-scope-access\t{}\tstatus={}\tread-only={}\taccess-started={}\tstale={}\treason=bookmark-resolved",
        report.path.display(),
        access.report.status.as_str(),
        access.report.read_only,
        access.report.access_started,
        access.report.stale
    );
    Ok(vec![access])
}

const fn read_only_intent(intent: AccessIntent) -> bool {
    matches!(
        intent,
        AccessIntent::Read | AccessIntent::Index | AccessIntent::Preview
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_mac::{VolumeDescriptor, VolumeKind};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn volume_reachability_preflight_refuses_unreachable_containing_volume() {
        let root = unique_temp_dir("gfm-access-unreachable-volume");
        let file = root.join("Preview.pdf");
        fs::write(&file, "%PDF-1.7\n").unwrap();
        let mut volume = VolumeDescriptor::for_path(&root).unwrap();
        volume.kind = VolumeKind::Network;
        volume.reachable = Some(false);
        let report = VolumeDiscoveryReport {
            volumes: vec![volume],
        };

        let err =
            preflight_volume_reachability_in_report(&file, &file, "quicklook preview", &report)
                .unwrap_err();

        assert!(matches!(err, GfmError::Permission { .. }));
        assert!(err
            .to_string()
            .contains("quicklook preview volume access blocked"));
        assert!(err.to_string().contains("unreachable volume network"));

        fs::remove_dir_all(root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "{}-{}-{}",
            prefix,
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
