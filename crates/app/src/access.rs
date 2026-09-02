use crate::{
    permission_refresh::{refresh_permission_state, PermissionRefreshAudience},
    runtime::default_security_bookmarks_path,
};
use gfm_mac::{
    AccessIntent, AccessProbeState, SecurityDecisionAction, SecurityScopedAccessReport,
    SecurityScopedBookmarkAccess, SecurityScopedBookmarkStore, SecurityWorkerAction,
    SecurityWorkerAdmissionReport, VolumeDescriptor, VolumeDiscoveryReport,
};
use gfm_types::{GfmError, Result};
use std::path::{Path, PathBuf};

pub(crate) struct ScopedAccessGuard {
    _accesses: Vec<SecurityScopedBookmarkAccess>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkerAdmissionRequest {
    pub(crate) worker: String,
    pub(crate) intent: AccessIntent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkerAdmissionVolumeGateReport {
    pub(crate) admission: SecurityWorkerAdmissionReport,
    pub(crate) volume_path: PathBuf,
    pub(crate) volume_report: VolumeDiscoveryReport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkerAdmissionsVolumeGateReport {
    pub(crate) admissions: Vec<SecurityWorkerAdmissionReport>,
    pub(crate) volume_path: PathBuf,
    pub(crate) volume_report: VolumeDiscoveryReport,
}

pub(crate) fn worker_admission_with_volume_gate_checked(
    path: &Path,
    intent: AccessIntent,
    worker: impl Into<String>,
    check_control: impl FnMut() -> Result<()>,
) -> Result<SecurityWorkerAdmissionReport> {
    Ok(worker_admission_volume_gate_report_checked(path, intent, worker, check_control)?.admission)
}

pub(crate) fn worker_admission_volume_gate_report_checked(
    path: &Path,
    intent: AccessIntent,
    worker: impl Into<String>,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<WorkerAdmissionVolumeGateReport> {
    check_control()?;
    let worker = worker.into();
    let volume_path = absolute_volume_probe_path(path);
    let volume_report =
        VolumeDiscoveryReport::for_containing_path_checked(&volume_path, &mut check_control)?;
    check_control()?;
    let admission = worker_admission_with_volume_report_for_probe(
        path,
        &volume_path,
        intent,
        worker.clone(),
        &volume_report,
    );
    if worker_admission_blocked_by_volume(&admission) {
        return Ok(WorkerAdmissionVolumeGateReport {
            admission,
            volume_path,
            volume_report,
        });
    }
    let _ = refresh_permission_state(PermissionRefreshAudience::Workers, &worker)?;
    check_control()?;
    let admission = worker_admission_with_volume_report_for_probe(
        path,
        &volume_path,
        intent,
        worker,
        &volume_report,
    );
    Ok(WorkerAdmissionVolumeGateReport {
        admission,
        volume_path,
        volume_report,
    })
}

pub(crate) fn worker_admission_with_volume_report(
    path: &Path,
    intent: AccessIntent,
    worker: impl Into<String>,
    volume_report: &VolumeDiscoveryReport,
) -> SecurityWorkerAdmissionReport {
    let volume_path = absolute_volume_probe_path(path);
    worker_admission_with_volume_report_for_probe(path, &volume_path, intent, worker, volume_report)
}

fn worker_admission_with_volume_report_for_probe(
    path: &Path,
    volume_path: &Path,
    intent: AccessIntent,
    worker: impl Into<String>,
    volume_report: &VolumeDiscoveryReport,
) -> SecurityWorkerAdmissionReport {
    let worker = worker.into();
    if let Some(block) = volume_access_block_in_report(volume_path, intent, &worker, volume_report)
    {
        let access = SecurityScopedAccessReport::blocked_before_filesystem_probe_with_state(
            path,
            intent,
            block.probe,
            &block.reason,
        );
        return SecurityWorkerAdmissionReport {
            worker,
            access,
            worker_action: SecurityWorkerAction::Deny,
            can_touch_filesystem: false,
            needs_bookmark_access: false,
            refresh_on_permission_change: true,
            reason: block.reason,
        };
    }
    let access = SecurityScopedAccessReport::evaluate(path, intent);
    access.worker_admission(worker)
}

#[cfg(test)]
pub(crate) fn worker_admissions_with_shared_volume_report_checked(
    path: &Path,
    requests: &[WorkerAdmissionRequest],
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<SecurityWorkerAdmissionReport>> {
    Ok(
        worker_admissions_volume_gate_report_checked(path, requests, &mut check_control)?
            .admissions,
    )
}

pub(crate) fn worker_admissions_volume_gate_report_checked(
    path: &Path,
    requests: &[WorkerAdmissionRequest],
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<WorkerAdmissionsVolumeGateReport> {
    check_control()?;
    let subject = worker_admission_fanout_subject(requests);
    let volume_path = absolute_volume_probe_path(path);
    let volume_report =
        VolumeDiscoveryReport::for_containing_path_checked(&volume_path, &mut check_control)?;
    check_control()?;
    let admissions = worker_admissions_with_volume_report(path, requests, &volume_report);
    if admissions.iter().all(worker_admission_blocked_by_volume) {
        return Ok(WorkerAdmissionsVolumeGateReport {
            admissions,
            volume_path,
            volume_report,
        });
    }
    let _ = refresh_permission_state(PermissionRefreshAudience::Workers, &subject)?;
    check_control()?;
    let admissions = worker_admissions_with_volume_report(path, requests, &volume_report);
    Ok(WorkerAdmissionsVolumeGateReport {
        admissions,
        volume_path,
        volume_report,
    })
}

pub(crate) fn worker_admissions_with_volume_report(
    path: &Path,
    requests: &[WorkerAdmissionRequest],
    volume_report: &VolumeDiscoveryReport,
) -> Vec<SecurityWorkerAdmissionReport> {
    let volume_path = absolute_volume_probe_path(path);
    worker_admissions_with_volume_report_for_probe(path, &volume_path, requests, volume_report)
}

fn worker_admissions_with_volume_report_for_probe(
    path: &Path,
    volume_path: &Path,
    requests: &[WorkerAdmissionRequest],
    volume_report: &VolumeDiscoveryReport,
) -> Vec<SecurityWorkerAdmissionReport> {
    requests
        .iter()
        .map(|request| {
            worker_admission_with_volume_report_for_probe(
                path,
                volume_path,
                request.intent,
                request.worker.clone(),
                volume_report,
            )
        })
        .collect()
}

fn worker_admission_fanout_subject(requests: &[WorkerAdmissionRequest]) -> String {
    const MAX_SUBJECT_CHARS: usize = 160;
    let mut subject = String::from("worker-admission-fanout");
    for request in requests {
        let candidate = format!(";{}:{}", request.worker, request.intent.as_str());
        if subject.len().saturating_add(candidate.len()) > MAX_SUBJECT_CHARS {
            subject.push_str(";...");
            break;
        }
        subject.push_str(&candidate);
    }
    subject
}

pub(crate) fn worker_admission_blocked_by_volume(
    admission: &SecurityWorkerAdmissionReport,
) -> bool {
    !admission.can_touch_filesystem && admission.reason.contains(" volume access blocked: ")
}

pub(crate) fn preflight_access_scope_checked(
    path: &Path,
    intent: AccessIntent,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<ScopedAccessGuard> {
    check_control()?;
    let volume_path = absolute_volume_probe_path(path);
    let volume_report =
        VolumeDiscoveryReport::for_containing_path_checked(&volume_path, &mut check_control)?;
    check_control()?;
    preflight_access_scope_checked_with_volume_report(
        path,
        intent,
        worker,
        &volume_report,
        check_control,
    )
}

pub(crate) fn preflight_access_scope_checked_with_volume_report(
    path: &Path,
    intent: AccessIntent,
    worker: &str,
    volume_report: &VolumeDiscoveryReport,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<ScopedAccessGuard> {
    check_control()?;
    let _ = refresh_permission_state(PermissionRefreshAudience::Workers, worker)?;
    check_control()?;
    let volume_path = absolute_volume_probe_path(path);
    preflight_volume_access_in_report(path, &volume_path, intent, worker, volume_report)?;
    check_control()?;
    let report = SecurityScopedAccessReport::evaluate(path, intent);
    check_control()?;
    eprintln!("{}", report.as_tsv());
    let admission = report.worker_admission(worker);
    eprintln!("{}", admission.as_tsv());
    match admission.worker_action {
        SecurityWorkerAction::Start => {
            let accesses = retained_security_accesses_checked(&report, &mut check_control)?;
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

pub(crate) fn preflight_volume_access_scope_with_report(
    path: &Path,
    intent: AccessIntent,
    worker: &str,
    volume_report: &VolumeDiscoveryReport,
) -> Result<()> {
    let volume_path = absolute_volume_probe_path(path);
    preflight_volume_access_in_report(path, &volume_path, intent, worker, volume_report)
}

fn preflight_volume_access_checked(
    path: &Path,
    intent: AccessIntent,
    worker: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    check_control()?;
    let volume_path = absolute_volume_probe_path(path);
    let report =
        VolumeDiscoveryReport::for_containing_path_checked(&volume_path, &mut check_control)?;
    check_control()?;
    preflight_volume_access_in_report(path, &volume_path, intent, worker, &report)
}

fn preflight_volume_access_in_report(
    user_path: &Path,
    volume_path: &Path,
    intent: AccessIntent,
    worker: &str,
    report: &VolumeDiscoveryReport,
) -> Result<()> {
    if let Some(block) = volume_access_block_in_report(volume_path, intent, worker, report) {
        return Err(GfmError::Permission {
            path: user_path.to_path_buf(),
            message: block.reason,
        });
    }
    Ok(())
}

struct VolumeAccessBlock {
    reason: String,
    probe: AccessProbeState,
}

fn volume_access_block_in_report(
    volume_path: &Path,
    intent: AccessIntent,
    worker: &str,
    report: &VolumeDiscoveryReport,
) -> Option<VolumeAccessBlock> {
    let volume = report.volume_for_path(volume_path)?;
    if volume.mount_state != gfm_mac::MountState::Mounted {
        return Some(VolumeAccessBlock {
            reason: format!(
            "{worker} volume access blocked: unmounted volume {}; label={}; root={}; stable-id={}; mount={}",
            volume.kind.as_str(),
            volume.label,
            volume.path.display(),
            volume.stable_identity,
            volume.mount_state.as_str()
        ),
            probe: AccessProbeState::Unknown,
        });
    }
    if volume.reachable == Some(false) {
        return Some(VolumeAccessBlock {
            reason: format!(
            "{worker} volume access blocked: unreachable volume {}; label={}; root={}; stable-id={}; mount={}",
            volume.kind.as_str(),
            volume.label,
            volume.path.display(),
            volume.stable_identity,
            volume.mount_state.as_str()
        ),
            probe: AccessProbeState::Unknown,
        });
    }
    if volume.platform_state_unavailable() {
        return Some(VolumeAccessBlock {
            reason: format!(
            "{worker} volume access blocked: unavailable volume {}; label={}; root={}; stable-id={}; mount={}; {}",
            volume.kind.as_str(),
            volume.label,
            volume.path.display(),
            volume.stable_identity,
            volume.mount_state.as_str(),
            volume_api_status_context(volume)
        ),
            probe: AccessProbeState::Unavailable,
        });
    }
    if mutating_intent(intent)
        && volume.read_only
        && !broad_system_root_allows_path(volume, volume_path, intent)
    {
        return Some(VolumeAccessBlock {
            reason: format!(
            "{worker} volume access blocked: read-only volume {}; label={}; root={}; stable-id={}; mount={}",
            volume.kind.as_str(),
            volume.label,
            volume.path.display(),
            volume.stable_identity,
            volume.mount_state.as_str()
        ),
            probe: AccessProbeState::Unknown,
        });
    }
    None
}

const fn mutating_intent(intent: AccessIntent) -> bool {
    matches!(intent, AccessIntent::Write | AccessIntent::Operate)
}

pub(crate) fn volume_api_status_context(volume: &VolumeDescriptor) -> String {
    format!(
        "native-status={}; native-reason={}; resource-status={}; resource-reason={}; mount-status={}; mount-reason={}",
        volume
            .native_status
            .map(gfm_mac::NativeVolumeStatus::as_str)
            .unwrap_or("-"),
        visible_volume_api_reason(volume.native_reason.as_deref()),
        volume
            .resource_status
            .map(gfm_mac::NativeVolumeStatus::as_str)
            .unwrap_or("-"),
        visible_volume_api_reason(volume.resource_reason.as_deref()),
        volume
            .mount_table_status
            .map(gfm_mac::NativeVolumeStatus::as_str)
            .unwrap_or("-"),
        visible_volume_api_reason(volume.mount_table_reason.as_deref())
    )
}

fn visible_volume_api_reason(reason: Option<&str>) -> &str {
    reason
        .filter(|reason| !reason.trim().is_empty())
        .unwrap_or("-")
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

fn retained_security_accesses_checked(
    report: &SecurityScopedAccessReport,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<SecurityScopedBookmarkAccess>> {
    if !report.bookmark_required || !matches!(report.action, SecurityDecisionAction::Allow) {
        return Ok(Vec::new());
    }
    check_control()?;
    let store = SecurityScopedBookmarkStore::new(default_security_bookmarks_path());
    retained_security_accesses_from_store_checked(report, &store, &mut check_control)
}

fn retained_security_accesses_from_store_checked(
    report: &SecurityScopedAccessReport,
    store: &SecurityScopedBookmarkStore,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<SecurityScopedBookmarkAccess>> {
    check_control()?;
    preflight_volume_access_checked(
        store.path(),
        AccessIntent::Read,
        "security bookmark store",
        &mut check_control,
    )?;
    check_control()?;
    let lookup = store.start_access_for_path_checked(
        &report.path,
        read_only_intent(report.intent),
        true,
        &mut check_control,
    )?;
    check_control()?;
    let Some(access) = lookup.access else {
        eprintln!(
            "{}",
            missing_security_scope_access_line(&report.path, read_only_intent(report.intent))
        );
        return Err(GfmError::Permission {
            path: report.path.clone(),
            message: format!(
                "retained security-scoped bookmark required before touching filesystem: {}",
                report.reason
            ),
        });
    };
    eprintln!("{}", resolved_security_scope_access_line(&access));
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
    check_control()?;
    Ok(vec![access])
}

const fn read_only_intent(intent: AccessIntent) -> bool {
    matches!(
        intent,
        AccessIntent::Read | AccessIntent::Index | AccessIntent::Preview
    )
}

fn missing_security_scope_access_line(path: &Path, read_only: bool) -> String {
    format!(
        "security-scope-access\t{}\tstatus=missing\tread-only={read_only}\taccess-started=false\treason=current-access-without-retained-bookmark",
        escape_path_field(path)
    )
}

fn resolved_security_scope_access_line(access: &SecurityScopedBookmarkAccess) -> String {
    format!(
        "security-scope-access\t{}\tstatus={}\tread-only={}\taccess-started={}\tstale={}\treason=bookmark-resolved",
        escape_path_field(&access.report.path),
        access.report.status.as_str(),
        access.report.read_only,
        access.report.access_started,
        access.report.stale
    )
}

fn escape_path_field(path: &Path) -> String {
    escape_field(&path.to_string_lossy())
}

fn escape_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
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

        let err = match retained_security_accesses_from_store_checked(&report, &store, || Ok(())) {
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
    fn missing_security_scope_access_line_escapes_control_characters_in_path() {
        let line = missing_security_scope_access_line(
            Path::new("/Users/me/Documents/Reports\tQ3\nDraft\rFinal.md"),
            true,
        );

        assert_eq!(line.lines().count(), 1, "{line}");
        assert!(line.starts_with(
            "security-scope-access\t/Users/me/Documents/Reports\\tQ3\\nDraft\\rFinal.md\tstatus=missing\t"
        ));
        assert_eq!(line.split('\t').count(), 6, "{line}");
    }

    #[test]
    fn resolved_security_scope_access_line_escapes_control_characters_in_path() {
        let root = unique_temp_dir("gfm-access-resolved-bookmark-line");
        let path = root.join("Documents\tQ3").join("Draft\nPlan\rFinal.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "plan").unwrap();
        let store = SecurityScopedBookmarkStore::new(root.join("bookmarks.tsv"));
        store
            .upsert(gfm_mac::SecurityScopedBookmark::create(&path, true).unwrap())
            .unwrap();
        let access = store
            .start_access_for_path(&path, true, true)
            .unwrap()
            .access
            .expect("matching bookmark access");

        let line = resolved_security_scope_access_line(&access);

        assert_eq!(line.lines().count(), 1, "{line}");
        assert!(
            line.contains("Documents\\tQ3/Draft\\nPlan\\rFinal.md\tstatus=resolved\t"),
            "{line}"
        );
        assert_eq!(line.split('\t').count(), 7, "{line}");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retained_bookmark_preflight_checked_honors_pre_cancelled_control_before_store_read() {
        let root = unique_temp_dir("gfm-access-cancelled-bookmark");
        let path = root.join("Documents").join("Plan.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "plan").unwrap();
        let store = SecurityScopedBookmarkStore::new(root.join("bookmarks.tsv"));
        let report = SecurityScopedAccessReport {
            path,
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

        let err = match retained_security_accesses_from_store_checked(&report, &store, || {
            Err(GfmError::Cancelled)
        }) {
            Ok(_) => {
                panic!("pre-cancelled retained bookmark preflight should stop before store read")
            }
            Err(err) => err,
        };

        assert_eq!(err, GfmError::Cancelled);
        assert!(!store.path().exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn access_scope_checked_honors_pre_cancelled_control_before_volume_discovery() {
        let root = unique_temp_dir("gfm-access-scope-volume-pre-cancel");
        let path = root.join("Preview.pdf");
        fs::write(&path, "%PDF-1.7\n").unwrap();

        let result =
            preflight_access_scope_checked(&path, AccessIntent::Preview, "preview", || {
                Err(GfmError::Cancelled)
            });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bookmark_store_volume_preflight_checked_honors_pre_cancelled_control() {
        let root = unique_temp_dir("gfm-access-bookmark-store-volume-pre-cancel");
        let store_path = root.join("bookmarks.tsv");

        let result = preflight_volume_access_checked(
            &store_path,
            AccessIntent::Read,
            "security bookmark store",
            || Err(GfmError::Cancelled),
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_admission_gate_refuses_unreachable_volume_before_filesystem_touch() {
        let root = unique_temp_dir("gfm-access-admission-unreachable-volume");
        fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
        let path = root.join("Preview.pdf");
        fs::write(&path, "%PDF-1.7\n").unwrap();

        let admission = worker_admission_with_volume_gate_checked(
            &path,
            AccessIntent::Preview,
            "preview worker",
            || Ok(()),
        )
        .unwrap();

        assert_eq!(admission.worker_action, SecurityWorkerAction::Deny);
        assert!(!admission.can_touch_filesystem);
        assert!(!admission.needs_bookmark_access);
        assert!(admission.refresh_on_permission_change);
        assert_eq!(admission.access.probe, gfm_mac::AccessProbeState::Unknown);
        assert_eq!(admission.access.action, SecurityDecisionAction::Deny);
        assert!(admission
            .reason
            .contains("preview worker volume access blocked"));
        assert!(admission.reason.contains("unreachable volume network"));
        assert!(admission.as_tsv().contains("\tworker-action=deny\t"));
        assert!(admission
            .as_tsv()
            .contains("\tcan-touch-filesystem=false\t"));
        assert!(worker_admission_blocked_by_volume(&admission));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_admission_gate_checked_honors_pre_cancelled_control() {
        let root = unique_temp_dir("gfm-access-admission-pre-cancel");
        let path = root.join("Preview.pdf");
        fs::write(&path, "%PDF-1.7\n").unwrap();

        let result = worker_admission_with_volume_gate_checked(
            &path,
            AccessIntent::Preview,
            "preview worker",
            || Err(GfmError::Cancelled),
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_admission_gate_does_not_probe_missing_path_on_blocked_volume() {
        let root = unique_temp_dir("gfm-access-admission-no-probe-volume");
        fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
        let path = root.join("Missing.pdf");

        let admission = worker_admission_with_volume_gate_checked(
            &path,
            AccessIntent::Preview,
            "preview worker",
            || Ok(()),
        )
        .unwrap();

        assert_eq!(admission.worker_action, SecurityWorkerAction::Deny);
        assert!(!admission.can_touch_filesystem);
        assert_eq!(admission.access.probe, gfm_mac::AccessProbeState::Unknown);
        assert_eq!(admission.access.action, SecurityDecisionAction::Deny);
        assert!(admission
            .access
            .reason
            .contains("preview worker volume access blocked"));
        assert!(admission.as_tsv().contains("\tprobe=unknown\t"));
        assert!(!admission.as_tsv().contains("\tprobe=missing\t"));
        assert!(worker_admission_blocked_by_volume(&admission));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_admission_volume_gate_report_returns_cached_volume_context() {
        let root = unique_temp_dir("gfm-access-admission-volume-report");
        let path = root.join("Preview.pdf");
        fs::write(&path, "%PDF-1.7\n").unwrap();

        let report = worker_admission_volume_gate_report_checked(
            &path,
            AccessIntent::Preview,
            "preview worker",
            || Ok(()),
        )
        .unwrap();

        assert_eq!(report.admission.worker, "preview worker");
        assert_eq!(report.admission.worker_action, SecurityWorkerAction::Start);
        assert!(report.admission.can_touch_filesystem);
        assert_eq!(report.volume_path, path);
        let volume = report
            .volume_report
            .volume_for_path(&report.volume_path)
            .expect("cached containing volume");
        assert!(!volume.stable_identity.is_empty());
        assert_eq!(volume.mount_state, gfm_mac::MountState::Mounted);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_admission_fanout_reuses_one_volume_gate_for_multiple_workers() {
        let root = unique_temp_dir("gfm-access-admission-fanout-unreachable");
        fs::write(root.join(".gfm-volume-kind"), "network-unreachable\n").unwrap();
        let path = root.join("Missing.pdf");
        let requests = [
            WorkerAdmissionRequest {
                worker: "index worker".to_string(),
                intent: AccessIntent::Index,
            },
            WorkerAdmissionRequest {
                worker: "preview worker".to_string(),
                intent: AccessIntent::Preview,
            },
            WorkerAdmissionRequest {
                worker: "operation worker".to_string(),
                intent: AccessIntent::Operate,
            },
        ];

        let admissions =
            worker_admissions_with_shared_volume_report_checked(&path, &requests, || Ok(()))
                .unwrap();

        assert_eq!(admissions.len(), requests.len());
        for (admission, request) in admissions.iter().zip(requests.iter()) {
            assert_eq!(admission.worker, request.worker);
            assert_eq!(admission.access.intent, request.intent);
            assert_eq!(admission.worker_action, SecurityWorkerAction::Deny);
            assert!(!admission.can_touch_filesystem);
            assert!(!admission.needs_bookmark_access);
            assert!(admission.refresh_on_permission_change);
            assert_eq!(admission.access.probe, gfm_mac::AccessProbeState::Unknown);
            assert_eq!(admission.access.action, SecurityDecisionAction::Deny);
            assert!(admission.reason.contains("unreachable volume network"));
            assert!(admission.as_tsv().contains("\tprobe=unknown\t"));
            assert!(!admission.as_tsv().contains("\tprobe=missing\t"));
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_admission_fanout_checked_honors_pre_cancelled_control() {
        let root = unique_temp_dir("gfm-access-admission-fanout-pre-cancel");
        let path = root.join("Preview.pdf");
        fs::write(&path, "%PDF-1.7\n").unwrap();
        let requests = [WorkerAdmissionRequest {
            worker: "index worker".to_string(),
            intent: AccessIntent::Index,
        }];

        let result = worker_admissions_with_shared_volume_report_checked(&path, &requests, || {
            Err(GfmError::Cancelled)
        });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_admission_fanout_accepts_precomputed_volume_report() {
        let root = unique_temp_dir("gfm-access-admission-fanout-report");
        let path = root.join("Missing.pdf");
        let mut volume = VolumeDescriptor::for_path(&root).unwrap();
        volume.kind = VolumeKind::Network;
        volume.reachable = Some(true);
        volume.native_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
        volume.native_reason = Some("DiskArbitration event session unavailable".to_string());
        volume.resource_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
        volume.resource_reason = Some("URL resource values unavailable".to_string());
        volume.mount_table_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
        volume.mount_table_reason = Some("mount table unavailable".to_string());
        let report = VolumeDiscoveryReport {
            volumes: vec![volume],
        };
        let requests = [
            WorkerAdmissionRequest {
                worker: "index worker".to_string(),
                intent: AccessIntent::Index,
            },
            WorkerAdmissionRequest {
                worker: "preview worker".to_string(),
                intent: AccessIntent::Preview,
            },
        ];

        let admissions = worker_admissions_with_volume_report(&path, &requests, &report);

        assert_eq!(admissions.len(), 2);
        for admission in admissions {
            assert_eq!(admission.worker_action, SecurityWorkerAction::Deny);
            assert!(!admission.can_touch_filesystem);
            assert!(admission.refresh_on_permission_change);
            assert_eq!(
                admission.access.probe,
                gfm_mac::AccessProbeState::Unavailable
            );
            assert!(admission.reason.contains("unavailable volume network"));
            assert!(admission.reason.contains("native-status=unavailable"));
            assert!(admission.as_tsv().contains("\tprobe=unavailable\t"));
            assert!(!admission.as_tsv().contains("\tprobe=unknown\t"));
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_admission_gate_refuses_stale_volume_before_filesystem_probe() {
        let root = unique_temp_dir("gfm-access-admission-stale-volume");
        let path = root.join("Missing.pdf");
        let mut volume = VolumeDescriptor::for_path(&root).unwrap();
        volume.kind = VolumeKind::Network;
        volume.mount_state = gfm_mac::MountState::Stale;
        volume.reachable = Some(true);
        let report = VolumeDiscoveryReport {
            volumes: vec![volume],
        };

        let admission = worker_admission_with_volume_report(
            &path,
            AccessIntent::Preview,
            "preview worker",
            &report,
        );

        assert_eq!(admission.worker_action, SecurityWorkerAction::Deny);
        assert!(!admission.can_touch_filesystem);
        assert!(!admission.needs_bookmark_access);
        assert!(admission.refresh_on_permission_change);
        assert_eq!(admission.access.probe, gfm_mac::AccessProbeState::Unknown);
        assert_eq!(admission.access.action, SecurityDecisionAction::Deny);
        assert!(admission
            .reason
            .contains("preview worker volume access blocked"));
        assert!(admission.reason.contains("unmounted volume network"));
        assert!(admission.reason.contains("mount=stale"));
        assert!(admission.as_tsv().contains("\tprobe=unknown\t"));
        assert!(!admission.as_tsv().contains("\tprobe=missing\t"));
        assert!(worker_admission_blocked_by_volume(&admission));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_admission_gate_refuses_unavailable_volume_api_state() {
        let root = unique_temp_dir("gfm-access-admission-unavailable-api");
        let path = root.join("Missing.pdf");
        let mut volume = VolumeDescriptor::for_path(&root).unwrap();
        volume.kind = VolumeKind::Network;
        volume.reachable = Some(true);
        volume.native_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
        volume.native_reason = Some("DiskArbitration event session unavailable".to_string());
        volume.resource_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
        volume.resource_reason = Some("URL resource values unavailable".to_string());
        volume.mount_table_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
        volume.mount_table_reason = Some("mount table unavailable".to_string());
        let report = VolumeDiscoveryReport {
            volumes: vec![volume],
        };

        let admission = worker_admission_with_volume_report(
            &path,
            AccessIntent::Preview,
            "preview worker",
            &report,
        );

        assert_eq!(admission.worker_action, SecurityWorkerAction::Deny);
        assert!(!admission.can_touch_filesystem);
        assert!(!admission.needs_bookmark_access);
        assert!(admission.refresh_on_permission_change);
        assert_eq!(
            admission.access.probe,
            gfm_mac::AccessProbeState::Unavailable
        );
        assert_eq!(admission.access.action, SecurityDecisionAction::Deny);
        assert!(admission
            .reason
            .contains("preview worker volume access blocked"));
        assert!(admission.reason.contains("unavailable volume network"));
        assert!(admission.reason.contains("native-status=unavailable"));
        assert!(admission
            .reason
            .contains("native-reason=DiskArbitration event session unavailable"));
        assert!(admission.reason.contains("resource-status=unavailable"));
        assert!(admission
            .reason
            .contains("resource-reason=URL resource values unavailable"));
        assert!(admission.reason.contains("mount-status=unavailable"));
        assert!(admission
            .reason
            .contains("mount-reason=mount table unavailable"));
        assert!(admission.as_tsv().contains("\tprobe=unavailable\t"));
        assert!(!admission.as_tsv().contains("\tprobe=unknown\t"));
        assert!(worker_admission_blocked_by_volume(&admission));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn volume_api_status_context_normalizes_blank_reasons() {
        let root = unique_temp_dir("gfm-access-api-blank-reasons");
        let mut volume = VolumeDescriptor::for_path(&root).unwrap();
        volume.native_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
        volume.native_reason = Some(" \t ".to_string());
        volume.resource_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
        volume.resource_reason = Some(String::new());
        volume.mount_table_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
        volume.mount_table_reason = Some("\n".to_string());

        let context = volume_api_status_context(&volume);

        assert_eq!(
            context,
            "native-status=unavailable; native-reason=-; resource-status=unavailable; resource-reason=-; mount-status=unavailable; mount-reason=-"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn volume_preflight_refuses_unavailable_volume_api_state() {
        let root = unique_temp_dir("gfm-access-preflight-unavailable-api");
        let file = root.join("Preview.pdf");
        let mut volume = VolumeDescriptor::for_path(&root).unwrap();
        volume.kind = VolumeKind::Network;
        volume.reachable = Some(true);
        volume.native_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
        volume.native_reason = Some("DiskArbitration event session unavailable".to_string());
        volume.resource_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
        volume.resource_reason = Some("URL resource values unavailable".to_string());
        volume.mount_table_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
        volume.mount_table_reason = Some("mount table unavailable".to_string());
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
        assert!(err.to_string().contains("unavailable volume network"));
        assert!(err.to_string().contains("native-status=unavailable"));
        assert!(err.to_string().contains("resource-status=unavailable"));
        assert!(err.to_string().contains("mount-status=unavailable"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn access_preflight_reuses_volume_report_before_filesystem_probe() {
        let root = unique_temp_dir("gfm-access-preflight-shared-volume-report");
        let file = root.join("Missing.pdf");
        let mut volume = VolumeDescriptor::for_path(&root).unwrap();
        volume.kind = VolumeKind::Network;
        volume.reachable = Some(true);
        volume.native_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
        volume.resource_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
        volume.mount_table_status = Some(gfm_mac::NativeVolumeStatus::Unavailable);
        let report = VolumeDiscoveryReport {
            volumes: vec![volume],
        };

        let err = match preflight_access_scope_checked_with_volume_report(
            &file,
            AccessIntent::Preview,
            "preview worker",
            &report,
            || Ok(()),
        ) {
            Ok(_) => panic!("shared volume preflight unexpectedly admitted unavailable volume"),
            Err(err) => err,
        };

        assert!(matches!(err, GfmError::Permission { .. }));
        assert!(err.to_string().contains("unavailable volume network"));
        assert!(err.to_string().contains("native-status=unavailable"));
        assert!(err.to_string().contains("resource-status=unavailable"));
        assert!(err.to_string().contains("mount-status=unavailable"));

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
        assert!(worker_admission_blocked_by_volume(&admission));

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
        assert!(!worker_admission_blocked_by_volume(&admission));

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
