use crate::{
    permission_refresh::{refresh_permission_state, PermissionRefreshAudience},
    runtime::default_security_bookmarks_path,
};
use gfm_mac::{
    AccessIntent, SecurityDecisionAction, SecurityScopedAccessReport, SecurityScopedBookmarkAccess,
    SecurityScopedBookmarkStore, SecurityWorkerAction, SecurityWorkerAdmissionReport,
    VolumeDiscoveryReport,
};
use gfm_types::{GfmError, Result};
use std::path::Path;

pub(crate) struct ScopedAccessGuard {
    _accesses: Vec<SecurityScopedBookmarkAccess>,
}

pub(crate) fn worker_admission_with_volume_gate(
    path: &Path,
    intent: AccessIntent,
    worker: impl Into<String>,
) -> SecurityWorkerAdmissionReport {
    let volume_path = absolute_volume_probe_path(path);
    let volume_report = VolumeDiscoveryReport::for_containing_path(&volume_path);
    worker_admission_with_volume_report(path, intent, worker, &volume_report)
}

pub(crate) fn worker_admission_with_volume_report(
    path: &Path,
    intent: AccessIntent,
    worker: impl Into<String>,
    volume_report: &VolumeDiscoveryReport,
) -> SecurityWorkerAdmissionReport {
    let worker = worker.into();
    let access = SecurityScopedAccessReport::evaluate(path, intent);
    let volume_path = absolute_volume_probe_path(path);
    if let Some(reason) =
        volume_access_block_reason_in_report(&volume_path, intent, &worker, volume_report)
    {
        return SecurityWorkerAdmissionReport {
            worker,
            access,
            worker_action: SecurityWorkerAction::Deny,
            can_touch_filesystem: false,
            needs_bookmark_access: false,
            refresh_on_permission_change: true,
            reason,
        };
    }
    access.worker_admission(worker)
}

pub(crate) fn preflight_access_scope(
    path: &Path,
    intent: AccessIntent,
    worker: &str,
) -> Result<ScopedAccessGuard> {
    let _ = refresh_permission_state(PermissionRefreshAudience::Workers, worker)?;
    preflight_volume_access(path, intent, worker)?;
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

pub(crate) fn preflight_volume_access_scope(
    path: &Path,
    intent: AccessIntent,
    worker: &str,
) -> Result<()> {
    preflight_volume_access(path, intent, worker)
}

fn preflight_volume_access(path: &Path, intent: AccessIntent, worker: &str) -> Result<()> {
    let volume_path = absolute_volume_probe_path(path);
    let report = VolumeDiscoveryReport::for_containing_path(&volume_path);
    preflight_volume_access_in_report(path, &volume_path, intent, worker, &report)
}

fn preflight_volume_access_in_report(
    user_path: &Path,
    volume_path: &Path,
    intent: AccessIntent,
    worker: &str,
    report: &VolumeDiscoveryReport,
) -> Result<()> {
    if let Some(reason) = volume_access_block_reason_in_report(volume_path, intent, worker, report)
    {
        return Err(GfmError::Permission {
            path: user_path.to_path_buf(),
            message: reason,
        });
    }
    Ok(())
}

fn volume_access_block_reason_in_report(
    volume_path: &Path,
    intent: AccessIntent,
    worker: &str,
    report: &VolumeDiscoveryReport,
) -> Option<String> {
    let volume = report.volume_for_path(volume_path)?;
    if volume.reachable == Some(false) {
        return Some(format!(
            "{worker} volume access blocked: unreachable volume {}; label={}; root={}; stable-id={}; mount={}",
            volume.kind.as_str(),
            volume.label,
            volume.path.display(),
            volume.stable_identity,
            volume.mount_state.as_str()
        ));
    }
    if mutating_intent(intent)
        && volume.read_only
        && !broad_system_root_allows_path(volume, volume_path, intent)
    {
        return Some(format!(
            "{worker} volume access blocked: read-only volume {}; label={}; root={}; stable-id={}; mount={}",
            volume.kind.as_str(),
            volume.label,
            volume.path.display(),
            volume.stable_identity,
            volume.mount_state.as_str()
        ));
    }
    None
}

const fn mutating_intent(intent: AccessIntent) -> bool {
    matches!(intent, AccessIntent::Write | AccessIntent::Operate)
}

fn broad_system_root_allows_path(
    volume: &gfm_mac::VolumeDescriptor,
    path: &Path,
    intent: AccessIntent,
) -> bool {
    volume.path == Path::new("/")
        && matches!(
            SecurityScopedAccessReport::evaluate(path, intent).action,
            SecurityDecisionAction::Allow
        )
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
    retained_security_accesses_from_store(report, &store)
}

fn retained_security_accesses_from_store(
    report: &SecurityScopedAccessReport,
    store: &SecurityScopedBookmarkStore,
) -> Result<Vec<SecurityScopedBookmarkAccess>> {
    preflight_volume_access(store.path(), AccessIntent::Read, "security bookmark store")?;
    let lookup =
        store.start_access_for_path(&report.path, read_only_intent(report.intent), true)?;
    let Some(access) = lookup.access else {
        eprintln!(
            "security-scope-access\t{}\tstatus=missing\tread-only={}\taccess-started=false\treason=current-access-without-retained-bookmark",
            report.path.display(),
            read_only_intent(report.intent)
        );
        return Err(GfmError::Permission {
            path: report.path.clone(),
            message: format!(
                "retained security-scoped bookmark required before touching filesystem: {}",
                report.reason
            ),
        });
    };
    eprintln!(
        "security-scope-access\t{}\tstatus={}\tread-only={}\taccess-started={}\tstale={}\treason=bookmark-resolved",
        report.path.display(),
        access.report.status.as_str(),
        access.report.read_only,
        access.report.access_started,
        access.report.stale
    );
    if access.report.stale || !access.report.access_started {
        return Err(GfmError::Permission {
            path: report.path.clone(),
            message: format!(
                "retained security-scoped bookmark did not provide current filesystem access: status={}; stale={}; access-started={}",
                access.report.status.as_str(),
                access.report.stale,
                access.report.access_started
            ),
        });
    }
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

        let err = preflight_volume_access_in_report(
            &file,
            &file,
            AccessIntent::Read,
            "quicklook preview",
            &report,
        )
        .unwrap_err();

        assert!(matches!(err, GfmError::Permission { .. }));
        assert!(err
            .to_string()
            .contains("quicklook preview volume access blocked"));
        assert!(err.to_string().contains("unreachable volume network"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retained_bookmark_preflight_fails_closed_when_store_has_no_match() {
        let root = unique_temp_dir("gfm-access-missing-bookmark");
        let path = root.join("Documents").join("Plan.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "plan").unwrap();
        let store = SecurityScopedBookmarkStore::new(root.join("bookmarks.tsv"));
        let report = SecurityScopedAccessReport {
            path: path.clone(),
            intent: AccessIntent::Index,
            scope: gfm_mac::ProtectedScope::Documents,
            probe: gfm_mac::AccessProbeState::Granted,
            mode: gfm_mac::SecurityAccessMode::SecurityScopedBookmark,
            action: SecurityDecisionAction::Allow,
            bookmark_required: true,
            can_read: true,
            can_write: false,
            least_privilege: true,
            reason: "path is readable now but should be retained with a security-scoped bookmark"
                .to_string(),
        };

        let err = match retained_security_accesses_from_store(&report, &store) {
            Ok(_) => panic!("missing retained bookmark must fail closed"),
            Err(err) => err,
        };

        assert!(matches!(err, GfmError::Permission { .. }));
        assert!(err
            .to_string()
            .contains("retained security-scoped bookmark required before touching filesystem"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_admission_gate_refuses_unreachable_volume_before_filesystem_touch() {
        let root = unique_temp_dir("gfm-access-admission-unreachable-volume");
        fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
        let path = root.join("Preview.pdf");
        fs::write(&path, "%PDF-1.7\n").unwrap();

        let admission =
            worker_admission_with_volume_gate(&path, AccessIntent::Preview, "preview worker");

        assert_eq!(admission.worker_action, SecurityWorkerAction::Deny);
        assert!(!admission.can_touch_filesystem);
        assert!(!admission.needs_bookmark_access);
        assert!(admission.refresh_on_permission_change);
        assert_eq!(admission.access.action, SecurityDecisionAction::Allow);
        assert!(admission
            .reason
            .contains("preview worker volume access blocked"));
        assert!(admission.reason.contains("unreachable volume network"));
        assert!(admission.as_tsv().contains("\tworker-action=deny\t"));
        assert!(admission
            .as_tsv()
            .contains("\tcan-touch-filesystem=false\t"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_admission_gate_refuses_write_intent_on_read_only_volume() {
        let root = unique_temp_dir("gfm-access-admission-read-only-volume");
        fs::write(
            root.join(".gfm-volume-kind"),
            "external-removable-read-only\n",
        )
        .unwrap();
        let path = root.join("Export.pdf");
        let mut volume = VolumeDescriptor::for_path(&root).unwrap();
        volume.kind = VolumeKind::External;
        volume.writable = false;
        volume.read_only = true;
        let report = VolumeDiscoveryReport {
            volumes: vec![volume],
        };

        let admission = worker_admission_with_volume_report(
            &path,
            AccessIntent::Write,
            "export worker",
            &report,
        );

        assert_eq!(admission.worker_action, SecurityWorkerAction::Deny);
        assert!(!admission.can_touch_filesystem);
        assert!(admission.refresh_on_permission_change);
        assert_eq!(admission.access.action, SecurityDecisionAction::Deny);
        assert!(admission
            .reason
            .contains("export worker volume access blocked"));
        assert!(admission.reason.contains("read-only volume external"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_admission_gate_allows_read_intent_on_read_only_volume() {
        let root = unique_temp_dir("gfm-access-admission-read-only-read");
        fs::write(
            root.join(".gfm-volume-kind"),
            "external-removable-read-only\n",
        )
        .unwrap();
        let path = root.join("Preview.pdf");
        fs::write(&path, "%PDF-1.7\n").unwrap();
        let volume = VolumeDescriptor::for_path(&root).unwrap();
        let report = VolumeDiscoveryReport {
            volumes: vec![volume],
        };

        let admission = worker_admission_with_volume_report(
            &path,
            AccessIntent::Preview,
            "preview worker",
            &report,
        );

        assert_eq!(admission.worker_action, SecurityWorkerAction::Start);
        assert!(admission.can_touch_filesystem);
        assert!(!admission.refresh_on_permission_change);
        assert!(admission
            .reason
            .contains("preview worker may start with filesystem access"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_admission_gate_allows_writable_path_under_broad_read_only_system_root() {
        let root = unique_temp_dir("gfm-access-admission-system-root");
        let mut volume = VolumeDescriptor::for_path("/").unwrap();
        volume.read_only = true;
        volume.writable = false;
        let report = VolumeDiscoveryReport {
            volumes: vec![volume],
        };

        let admission =
            worker_admission_with_volume_report(&root, AccessIntent::Write, "journal", &report);

        assert_eq!(admission.worker_action, SecurityWorkerAction::Start);
        assert!(admission.can_touch_filesystem);
        assert!(!admission.refresh_on_permission_change);

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
