use crate::{
    permission_refresh::{refresh_permission_state, PermissionRefreshAudience},
    runtime::default_security_bookmarks_path,
};
use gfm_mac::{
    AccessIntent, SecurityDecisionAction, SecurityScopedAccessReport, SecurityScopedBookmarkAccess,
    SecurityScopedBookmarkStore, SecurityWorkerAction,
};
use gfm_types::{GfmError, Result};
use std::path::Path;

pub(crate) fn preflight_access(path: &Path, intent: AccessIntent, worker: &str) -> Result<()> {
    let _ = preflight_access_scope(path, intent, worker)?;
    Ok(())
}

pub(crate) struct ScopedAccessGuard {
    _accesses: Vec<SecurityScopedBookmarkAccess>,
}

pub(crate) fn preflight_access_scope(
    path: &Path,
    intent: AccessIntent,
    worker: &str,
) -> Result<ScopedAccessGuard> {
    let _ = refresh_permission_state(PermissionRefreshAudience::Workers, worker)?;
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
