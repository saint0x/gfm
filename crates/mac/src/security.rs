use gfm_types::{GfmError, Result};
use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

mod bookmark;

pub use bookmark::{
    SecurityScopedBookmark, SecurityScopedBookmarkAccess, SecurityScopedBookmarkAccessLookup,
    SecurityScopedBookmarkLookup, SecurityScopedBookmarkRecord, SecurityScopedBookmarkReport,
    SecurityScopedBookmarkResolution, SecurityScopedBookmarkStatus, SecurityScopedBookmarkStore,
    SecurityScopedBookmarkStoreReport,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessIntent {
    Read,
    Write,
    Index,
    Preview,
    Operate,
}

impl AccessIntent {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Index => "index",
            Self::Preview => "preview",
            Self::Operate => "operate",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "read" => Ok(Self::Read),
            "write" => Ok(Self::Write),
            "index" => Ok(Self::Index),
            "preview" => Ok(Self::Preview),
            "operate" => Ok(Self::Operate),
            other => Err(GfmError::Format(format!(
                "access intent must be read, write, index, preview, or operate; got `{other}`"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectedScope {
    None,
    Desktop,
    Documents,
    Downloads,
    Mail,
    Photos,
    FullDiskAccess,
    ExternalVolume,
    NetworkVolume,
}

impl ProtectedScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Desktop => "desktop",
            Self::Documents => "documents",
            Self::Downloads => "downloads",
            Self::Mail => "mail",
            Self::Photos => "photos",
            Self::FullDiskAccess => "full-disk-access",
            Self::ExternalVolume => "external-volume",
            Self::NetworkVolume => "network-volume",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessProbeState {
    Granted,
    Missing,
    Denied,
    Unavailable,
    Unknown,
}

impl AccessProbeState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Granted => "granted",
            Self::Missing => "missing",
            Self::Denied => "denied",
            Self::Unavailable => "unavailable",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityAccessMode {
    PlainFilesystem,
    SecurityScopedBookmark,
    FullDiskAccess,
    DegradedMetadataOnly,
    Denied,
}

impl SecurityAccessMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PlainFilesystem => "plain-filesystem",
            Self::SecurityScopedBookmark => "security-scoped-bookmark",
            Self::FullDiskAccess => "full-disk-access",
            Self::DegradedMetadataOnly => "degraded-metadata-only",
            Self::Denied => "denied",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityDecisionAction {
    Allow,
    Prompt,
    Degrade,
    Deny,
}

impl SecurityDecisionAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Prompt => "prompt",
            Self::Degrade => "degrade",
            Self::Deny => "deny",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityWorkerAction {
    Start,
    MetadataOnly,
    Prompt,
    Deny,
}

impl SecurityWorkerAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::MetadataOnly => "metadata-only",
            Self::Prompt => "prompt",
            Self::Deny => "deny",
        }
    }

    pub const fn can_touch_filesystem(self) -> bool {
        matches!(self, Self::Start)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityScopedAccessReport {
    pub path: PathBuf,
    pub intent: AccessIntent,
    pub scope: ProtectedScope,
    pub probe: AccessProbeState,
    pub mode: SecurityAccessMode,
    pub action: SecurityDecisionAction,
    pub bookmark_required: bool,
    pub can_read: bool,
    pub can_write: bool,
    pub least_privilege: bool,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityWorkerAdmissionReport {
    pub worker: String,
    pub access: SecurityScopedAccessReport,
    pub worker_action: SecurityWorkerAction,
    pub can_touch_filesystem: bool,
    pub needs_bookmark_access: bool,
    pub refresh_on_permission_change: bool,
    pub reason: String,
}

impl SecurityScopedAccessReport {
    pub fn evaluate(path: impl AsRef<Path>, intent: AccessIntent) -> Self {
        let path = absolute_access_path(path.as_ref());
        let scope = protected_scope(&path);
        let probe = probe_path(&path, intent);
        let can_read = matches!(probe, AccessProbeState::Granted) && read_intent(intent);
        let can_write = matches!(probe, AccessProbeState::Granted) && write_intent(intent);
        let bookmark_required = requires_bookmark(scope, intent);
        let (mode, action, reason) = decide(scope, probe, intent, bookmark_required);
        let least_privilege = least_privilege(mode, intent);

        Self {
            path,
            intent,
            scope,
            probe,
            mode,
            action,
            bookmark_required,
            can_read,
            can_write,
            least_privilege,
            reason,
        }
    }

    pub fn blocked_before_filesystem_probe(
        path: impl AsRef<Path>,
        intent: AccessIntent,
        reason: impl Into<String>,
    ) -> Self {
        Self::blocked_before_filesystem_probe_with_state(
            path,
            intent,
            AccessProbeState::Unknown,
            reason,
        )
    }

    pub fn blocked_before_filesystem_probe_with_state(
        path: impl AsRef<Path>,
        intent: AccessIntent,
        probe: AccessProbeState,
        reason: impl Into<String>,
    ) -> Self {
        let path = absolute_access_path_without_filesystem_probe(path.as_ref());
        let scope = protected_scope_without_filesystem_probe(&path);
        Self {
            path,
            intent,
            scope,
            probe,
            mode: SecurityAccessMode::Denied,
            action: SecurityDecisionAction::Deny,
            bookmark_required: false,
            can_read: false,
            can_write: false,
            least_privilege: true,
            reason: reason.into(),
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "security-scope\t{}\tintent={}\tscope={}\tprobe={}\tmode={}\taction={}\tbookmark-required={}\tcan-read={}\tcan-write={}\tleast-privilege={}\treason={}",
            escape_path_field(&self.path),
            self.intent.as_str(),
            self.scope.as_str(),
            self.probe.as_str(),
            self.mode.as_str(),
            self.action.as_str(),
            self.bookmark_required,
            self.can_read,
            self.can_write,
            self.least_privilege,
            escape_field(&self.reason),
        )
    }

    pub fn worker_admission(&self, worker: impl Into<String>) -> SecurityWorkerAdmissionReport {
        SecurityWorkerAdmissionReport::from_access_report(worker, self.clone())
    }
}

impl SecurityWorkerAdmissionReport {
    pub fn evaluate(
        path: impl AsRef<Path>,
        intent: AccessIntent,
        worker: impl Into<String>,
    ) -> Self {
        SecurityScopedAccessReport::evaluate(path, intent).worker_admission(worker)
    }

    pub fn from_access_report(
        worker: impl Into<String>,
        access: SecurityScopedAccessReport,
    ) -> Self {
        let worker = worker.into();
        let worker_action = match access.action {
            SecurityDecisionAction::Allow => SecurityWorkerAction::Start,
            SecurityDecisionAction::Degrade => SecurityWorkerAction::MetadataOnly,
            SecurityDecisionAction::Prompt => SecurityWorkerAction::Prompt,
            SecurityDecisionAction::Deny => SecurityWorkerAction::Deny,
        };
        let can_touch_filesystem = worker_action.can_touch_filesystem();
        let needs_bookmark_access = can_touch_filesystem && access.bookmark_required;
        let refresh_on_permission_change = matches!(
            worker_action,
            SecurityWorkerAction::MetadataOnly | SecurityWorkerAction::Prompt
        ) || needs_bookmark_access
            || matches!(
                access.probe,
                AccessProbeState::Denied
                    | AccessProbeState::Unavailable
                    | AccessProbeState::Unknown
            );
        let reason = match worker_action {
            SecurityWorkerAction::Start if needs_bookmark_access => {
                format!("{worker} may start after retained security-scoped bookmark access")
            }
            SecurityWorkerAction::Start => format!("{worker} may start with filesystem access"),
            SecurityWorkerAction::MetadataOnly => {
                format!("{worker} must avoid file IO and publish metadata-only state")
            }
            SecurityWorkerAction::Prompt => {
                format!("{worker} must wait for permission prompt orchestration")
            }
            SecurityWorkerAction::Deny => format!("{worker} access is denied"),
        };

        Self {
            worker,
            access,
            worker_action,
            can_touch_filesystem,
            needs_bookmark_access,
            refresh_on_permission_change,
            reason,
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "security-worker-admission\tworker={}\tpath={}\tintent={}\tscope={}\tprobe={}\tmode={}\taccess-action={}\tworker-action={}\tcan-touch-filesystem={}\tbookmark-access={}\trefresh-on-permission-change={}\treason={}",
            escape_field(&self.worker),
            escape_path_field(&self.access.path),
            self.access.intent.as_str(),
            self.access.scope.as_str(),
            self.access.probe.as_str(),
            self.access.mode.as_str(),
            self.access.action.as_str(),
            self.worker_action.as_str(),
            self.can_touch_filesystem,
            self.needs_bookmark_access,
            self.refresh_on_permission_change,
            escape_field(&self.reason),
        )
    }
}

fn decide(
    scope: ProtectedScope,
    probe: AccessProbeState,
    intent: AccessIntent,
    bookmark_required: bool,
) -> (SecurityAccessMode, SecurityDecisionAction, String) {
    match probe {
        AccessProbeState::Granted if bookmark_required => (
            SecurityAccessMode::SecurityScopedBookmark,
            SecurityDecisionAction::Allow,
            "path is readable now but should be retained with a security-scoped bookmark"
                .to_string(),
        ),
        AccessProbeState::Granted => (
            SecurityAccessMode::PlainFilesystem,
            SecurityDecisionAction::Allow,
            "path is accessible through least-privilege filesystem access".to_string(),
        ),
        AccessProbeState::Missing => (
            SecurityAccessMode::Denied,
            SecurityDecisionAction::Deny,
            "path is not present on this host".to_string(),
        ),
        AccessProbeState::Denied if requires_full_disk_access(scope) => (
            SecurityAccessMode::FullDiskAccess,
            SecurityDecisionAction::Prompt,
            "protected root requires Full Disk Access guidance".to_string(),
        ),
        AccessProbeState::Denied if bookmark_required => (
            SecurityAccessMode::SecurityScopedBookmark,
            SecurityDecisionAction::Prompt,
            "access denied; request a user-selected security-scoped bookmark".to_string(),
        ),
        AccessProbeState::Denied
            if matches!(intent, AccessIntent::Index | AccessIntent::Preview) =>
        {
            (
                SecurityAccessMode::DegradedMetadataOnly,
                SecurityDecisionAction::Degrade,
                "access denied; continue with metadata-only degraded mode".to_string(),
            )
        }
        AccessProbeState::Denied => (
            SecurityAccessMode::Denied,
            SecurityDecisionAction::Deny,
            "access denied and no lower-privilege fallback is valid".to_string(),
        ),
        AccessProbeState::Unavailable => (
            SecurityAccessMode::Denied,
            SecurityDecisionAction::Deny,
            "access probe failed because the host filesystem or permission API was unavailable"
                .to_string(),
        ),
        AccessProbeState::Unknown => (
            SecurityAccessMode::DegradedMetadataOnly,
            SecurityDecisionAction::Degrade,
            "access probe was inconclusive; avoid blocking UI and retry through a scoped worker"
                .to_string(),
        ),
    }
}

fn protected_scope(path: &Path) -> ProtectedScope {
    if path.starts_with("/Volumes") {
        return ProtectedScope::ExternalVolume;
    }
    if path.starts_with("/Network") {
        return ProtectedScope::NetworkVolume;
    }

    let Some(home) = home_dir() else {
        return ProtectedScope::None;
    };
    protected_scope_for_home(path, &home)
}

fn protected_scope_without_filesystem_probe(path: &Path) -> ProtectedScope {
    if path.starts_with("/Volumes") {
        return ProtectedScope::ExternalVolume;
    }
    if path.starts_with("/Network") {
        return ProtectedScope::NetworkVolume;
    }
    let Some(home) = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| normalize_access_path(&path))
    else {
        return ProtectedScope::None;
    };
    protected_scope_for_home(path, &home)
}

fn protected_scope_for_home(path: &Path, home: &Path) -> ProtectedScope {
    if within(path, &home.join("Library/Mail")) {
        return ProtectedScope::FullDiskAccess;
    }
    if within(
        path,
        &home.join("Library/Group Containers/group.com.apple.mail"),
    ) {
        return ProtectedScope::Mail;
    }
    if within(path, &home.join("Desktop")) {
        return ProtectedScope::Desktop;
    }
    if within(path, &home.join("Documents")) {
        return ProtectedScope::Documents;
    }
    if within(path, &home.join("Downloads")) {
        return ProtectedScope::Downloads;
    }
    if within(path, &home.join("Pictures/Photos Library.photoslibrary")) {
        return ProtectedScope::Photos;
    }
    ProtectedScope::None
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| absolute_canonical_access_path(&path))
}

fn absolute_access_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    absolute_canonical_access_path(&absolute)
}

fn absolute_access_path_without_filesystem_probe(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    normalize_access_path(&absolute)
}

fn absolute_canonical_access_path(path: &Path) -> PathBuf {
    let normalized = normalize_access_path(path);
    canonicalize_existing_prefix(&normalized)
}

fn canonicalize_existing_prefix(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return normalize_access_path(&canonical);
    }

    let mut existing = path.to_path_buf();
    let mut tail = Vec::<OsString>::new();
    while !existing.as_os_str().is_empty() {
        if let Ok(canonical) = fs::canonicalize(&existing) {
            let mut rebuilt = normalize_access_path(&canonical);
            for component in tail.iter().rev() {
                rebuilt.push(component);
            }
            return normalize_access_path(&rebuilt);
        }
        let Some(name) = existing.file_name().map(OsString::from) else {
            break;
        };
        tail.push(name);
        if !existing.pop() {
            break;
        }
    }

    path.to_path_buf()
}

fn normalize_access_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir | Component::Normal(_) => normalized.push(component.as_os_str()),
        }
    }
    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

fn within(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

fn probe_path(path: &Path, intent: AccessIntent) -> AccessProbeState {
    if write_intent(intent) {
        return probe_write(path);
    }
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => match fs::read_dir(path) {
            Ok(_) => AccessProbeState::Granted,
            Err(err) => probe_error(err.kind()),
        },
        Ok(_) => match fs::File::open(path) {
            Ok(_) => AccessProbeState::Granted,
            Err(err) => probe_error(err.kind()),
        },
        Err(err) => probe_error(err.kind()),
    }
}

fn probe_write(path: &Path) -> AccessProbeState {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => probe_create_in_directory(path),
        Ok(_) => match fs::OpenOptions::new().append(true).open(path) {
            Ok(_) => AccessProbeState::Granted,
            Err(err) => probe_error(err.kind()),
        },
        Err(err) if err.kind() == ErrorKind::NotFound => probe_create_in_parent(path),
        Err(err) => probe_error(err.kind()),
    }
}

fn probe_create_in_parent(path: &Path) -> AccessProbeState {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return AccessProbeState::Missing;
    };
    match fs::metadata(parent) {
        Ok(metadata) if metadata.is_dir() => probe_create_in_directory(parent),
        Ok(_) => AccessProbeState::Denied,
        Err(err) => probe_error(err.kind()),
    }
}

fn probe_create_in_directory(path: &Path) -> AccessProbeState {
    let probe = path.join(write_probe_file_name());
    match fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = fs::remove_file(probe);
            AccessProbeState::Granted
        }
        Err(err) => probe_error(err.kind()),
    }
}

fn write_probe_file_name() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        ".gfm-write-probe-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn probe_error(kind: ErrorKind) -> AccessProbeState {
    match kind {
        ErrorKind::NotFound => AccessProbeState::Missing,
        ErrorKind::PermissionDenied => AccessProbeState::Denied,
        _ => AccessProbeState::Unavailable,
    }
}

fn requires_bookmark(scope: ProtectedScope, intent: AccessIntent) -> bool {
    matches!(
        scope,
        ProtectedScope::Desktop
            | ProtectedScope::Documents
            | ProtectedScope::Downloads
            | ProtectedScope::ExternalVolume
            | ProtectedScope::NetworkVolume
    ) && matches!(
        intent,
        AccessIntent::Read
            | AccessIntent::Write
            | AccessIntent::Index
            | AccessIntent::Preview
            | AccessIntent::Operate
    )
}

fn requires_full_disk_access(scope: ProtectedScope) -> bool {
    matches!(
        scope,
        ProtectedScope::Mail | ProtectedScope::Photos | ProtectedScope::FullDiskAccess
    )
}

fn least_privilege(mode: SecurityAccessMode, intent: AccessIntent) -> bool {
    match mode {
        SecurityAccessMode::PlainFilesystem => true,
        SecurityAccessMode::SecurityScopedBookmark => {
            matches!(
                intent,
                AccessIntent::Read
                    | AccessIntent::Write
                    | AccessIntent::Index
                    | AccessIntent::Preview
                    | AccessIntent::Operate
            )
        }
        SecurityAccessMode::DegradedMetadataOnly => {
            matches!(intent, AccessIntent::Index | AccessIntent::Preview)
        }
        SecurityAccessMode::FullDiskAccess => false,
        SecurityAccessMode::Denied => true,
    }
}

fn read_intent(intent: AccessIntent) -> bool {
    matches!(
        intent,
        AccessIntent::Read | AccessIntent::Index | AccessIntent::Preview | AccessIntent::Operate
    )
}

fn write_intent(intent: AccessIntent) -> bool {
    matches!(intent, AccessIntent::Write | AccessIntent::Operate)
}

fn escape_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

fn escape_path_field(path: &Path) -> String {
    escape_field(&path.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
    static CWD_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn plain_access_allows_unprotected_read() {
        let root = temp_root("security-plain");
        let path = root.join("note.md");
        fs::write(&path, "note").unwrap();

        let report = SecurityScopedAccessReport::evaluate(&path, AccessIntent::Read);

        assert_eq!(report.scope, ProtectedScope::None);
        assert_eq!(report.mode, SecurityAccessMode::PlainFilesystem);
        assert_eq!(report.action, SecurityDecisionAction::Allow);
        assert!(report.least_privilege);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn documents_named_temp_directory_does_not_require_bookmark_retention() {
        let root = temp_root("security-documents");
        let path = root.join("Documents").join("Plan.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "plan").unwrap();

        let report = SecurityScopedAccessReport::evaluate(&path, AccessIntent::Read);

        assert_eq!(report.scope, ProtectedScope::None);
        assert_eq!(report.mode, SecurityAccessMode::PlainFilesystem);
        assert_eq!(report.action, SecurityDecisionAction::Allow);
        assert!(!report.bookmark_required);
        assert!(report.as_tsv().contains("bookmark-required=false"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn protected_documents_scope_is_limited_to_home_documents() {
        let home = PathBuf::from("/Users/me");

        assert_eq!(
            protected_scope_for_home(Path::new("/Users/me/Documents/Plan.md"), &home),
            ProtectedScope::Documents
        );
        assert_eq!(
            protected_scope_for_home(Path::new("/private/tmp/Documents/Plan.md"), &home),
            ProtectedScope::None
        );
    }

    #[test]
    fn relative_access_paths_are_absolutized_and_normalized_before_scope_checks() {
        let _cwd = CWD_LOCK.lock().unwrap();
        let root = temp_root("security-relative-access");
        let home = root.join("home");
        let documents = home.join("Documents");
        let logical_path = documents.join("Plan.md");
        fs::create_dir_all(&documents).unwrap();
        fs::write(&logical_path, "plan").unwrap();
        let home = fs::canonicalize(&home).unwrap();
        let path = fs::canonicalize(&logical_path).unwrap();
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(&home).unwrap();

        let absolute = absolute_access_path(Path::new("./Documents/../Documents/Plan.md"));
        let report = SecurityScopedAccessReport::evaluate(
            "./Documents/../Documents/Plan.md",
            AccessIntent::Read,
        );
        let canonical_path = fs::canonicalize(&path).unwrap();
        let canonical_home = fs::canonicalize(&home).unwrap();

        assert_eq!(absolute, canonical_path);
        assert_eq!(
            protected_scope_for_home(&absolute, &canonical_home),
            ProtectedScope::Documents
        );
        assert_eq!(report.path, canonical_path);
        assert_eq!(report.probe, AccessProbeState::Granted);

        std::env::set_current_dir(previous).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn protected_preview_and_index_require_retained_bookmarks_when_accessible() {
        let home = temp_root("security-protected-preview-home");
        let documents = home.join("Documents");
        let path = documents.join("Plan.pdf");
        fs::create_dir_all(&documents).unwrap();
        fs::write(&path, "%PDF-1.7\n").unwrap();

        let preview = evaluate_for_home(&path, AccessIntent::Preview, &home);
        let index = evaluate_for_home(&path, AccessIntent::Index, &home);

        assert_eq!(preview.scope, ProtectedScope::Documents);
        assert_eq!(preview.action, SecurityDecisionAction::Allow);
        assert!(preview.bookmark_required);
        assert_eq!(preview.mode, SecurityAccessMode::SecurityScopedBookmark);
        assert!(preview.least_privilege);
        assert!(index.bookmark_required);
        assert_eq!(index.mode, SecurityAccessMode::SecurityScopedBookmark);

        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn missing_path_denies_without_prompting_for_more_privilege() {
        let root = temp_root("security-missing");
        let path = root.join("Missing.md");

        let report = SecurityScopedAccessReport::evaluate(&path, AccessIntent::Read);

        assert_eq!(report.probe, AccessProbeState::Missing);
        assert_eq!(report.mode, SecurityAccessMode::Denied);
        assert_eq!(report.action, SecurityDecisionAction::Deny);
        assert!(report.least_privilege);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unavailable_probe_failures_deny_without_claiming_unknown_state() {
        let root = temp_root("security-unavailable-probe");
        let path = root.join("permission-probe-unavailable".repeat(64));

        let report = SecurityScopedAccessReport::evaluate(&path, AccessIntent::Preview);
        let admission = report.worker_admission("preview worker");

        assert_eq!(report.probe, AccessProbeState::Unavailable);
        assert_eq!(report.mode, SecurityAccessMode::Denied);
        assert_eq!(report.action, SecurityDecisionAction::Deny);
        assert!(!report.can_read);
        assert!(!report.can_write);
        assert_eq!(admission.worker_action, SecurityWorkerAction::Deny);
        assert!(!admission.can_touch_filesystem);
        assert!(admission.refresh_on_permission_change);
        assert!(report.as_tsv().contains("\tprobe=unavailable\t"));
        assert!(!report.as_tsv().contains("\tprobe=unknown\t"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unavailable_write_probe_failures_deny_without_append_fallback() {
        let root = temp_root("security-unavailable-write-probe");
        let path = root.join("permission-write-probe-unavailable".repeat(64));

        let report = SecurityScopedAccessReport::evaluate(&path, AccessIntent::Write);

        assert_eq!(report.probe, AccessProbeState::Unavailable);
        assert_eq!(report.mode, SecurityAccessMode::Denied);
        assert_eq!(report.action, SecurityDecisionAction::Deny);
        assert!(!report.can_write);
        assert!(report.as_tsv().contains("\tprobe=unavailable\t"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn write_probe_allows_missing_file_when_parent_is_writable() {
        let root = temp_root("security-write-missing-creatable");
        let path = root.join("New.md");

        let report = SecurityScopedAccessReport::evaluate(&path, AccessIntent::Write);

        assert_eq!(report.probe, AccessProbeState::Granted);
        assert_eq!(report.mode, SecurityAccessMode::PlainFilesystem);
        assert_eq!(report.action, SecurityDecisionAction::Allow);
        assert!(report.can_write);
        assert!(!path.exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn protected_write_probe_allows_missing_file_with_retained_bookmark_requirement() {
        let home = temp_root("security-write-missing-protected-home");
        let documents = home.join("Documents");
        fs::create_dir_all(&documents).unwrap();
        let path = documents.join("New.md");

        let report = evaluate_for_home(&path, AccessIntent::Write, &home);

        assert_eq!(report.scope, ProtectedScope::Documents);
        assert_eq!(report.probe, AccessProbeState::Granted);
        assert_eq!(report.mode, SecurityAccessMode::SecurityScopedBookmark);
        assert_eq!(report.action, SecurityDecisionAction::Allow);
        assert!(report.bookmark_required);
        assert!(report.can_write);
        assert!(!path.exists());

        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn write_probe_keeps_missing_state_when_parent_is_missing() {
        let root = temp_root("security-write-missing-parent");
        let path = root.join("Missing").join("New.md");

        let report = SecurityScopedAccessReport::evaluate(&path, AccessIntent::Write);

        assert_eq!(report.probe, AccessProbeState::Missing);
        assert_eq!(report.mode, SecurityAccessMode::Denied);
        assert_eq!(report.action, SecurityDecisionAction::Deny);
        assert!(!report.can_write);
        assert!(!path.exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn blocked_before_probe_report_does_not_claim_missing_path_probe() {
        let path = PathBuf::from("/Volumes/Remote/Missing.pdf");

        let report = SecurityScopedAccessReport::blocked_before_filesystem_probe(
            &path,
            AccessIntent::Preview,
            "preview worker volume access blocked: unreachable volume network",
        );

        assert_eq!(report.path, path);
        assert_eq!(report.scope, ProtectedScope::ExternalVolume);
        assert_eq!(report.probe, AccessProbeState::Unknown);
        assert_eq!(report.mode, SecurityAccessMode::Denied);
        assert_eq!(report.action, SecurityDecisionAction::Deny);
        assert!(!report.bookmark_required);
        assert!(!report.can_read);
        assert!(!report.can_write);
        assert!(report.least_privilege);
        assert!(report.as_tsv().contains("\tprobe=unknown\t"));
        assert!(!report.as_tsv().contains("\tprobe=missing\t"));
    }

    #[test]
    fn blocked_before_probe_report_preserves_typed_unavailable_state() {
        let path = PathBuf::from("/Volumes/Remote/Missing.pdf");

        let report = SecurityScopedAccessReport::blocked_before_filesystem_probe_with_state(
            &path,
            AccessIntent::Preview,
            AccessProbeState::Unavailable,
            "preview worker volume access blocked: unavailable volume network",
        );

        assert_eq!(report.path, path);
        assert_eq!(report.scope, ProtectedScope::ExternalVolume);
        assert_eq!(report.probe, AccessProbeState::Unavailable);
        assert_eq!(report.mode, SecurityAccessMode::Denied);
        assert_eq!(report.action, SecurityDecisionAction::Deny);
        assert!(!report.bookmark_required);
        assert!(report.as_tsv().contains("\tprobe=unavailable\t"));
        assert!(!report.as_tsv().contains("\tprobe=unknown\t"));
    }

    #[test]
    fn security_scope_tsv_escapes_control_characters_in_path_fields() {
        let report = SecurityScopedAccessReport::blocked_before_filesystem_probe(
            PathBuf::from("/Users/me/Documents/Reports\tQ3\nDraft.md"),
            AccessIntent::Preview,
            "preview worker volume access blocked: unreachable volume network",
        );

        let tsv = report.as_tsv();

        assert_eq!(tsv.lines().count(), 1, "{tsv}");
        assert!(tsv.starts_with(
            "security-scope\t/Users/me/Documents/Reports\\tQ3\\nDraft.md\tintent=preview\t"
        ));
        assert_eq!(tsv.split('\t').count(), 12, "{tsv}");
    }

    #[test]
    fn worker_admission_tsv_escapes_control_characters_in_path_fields() {
        let report = SecurityScopedAccessReport::blocked_before_filesystem_probe(
            PathBuf::from("/Users/me/Documents/Reports\tQ3\nDraft.md"),
            AccessIntent::Preview,
            "preview worker volume access blocked: unreachable volume network",
        );

        let tsv = report.worker_admission("preview\tworker").as_tsv();

        assert_eq!(tsv.lines().count(), 1, "{tsv}");
        assert!(tsv.contains("worker=preview\\tworker\tpath=/Users/me/Documents/Reports\\tQ3\\nDraft.md\tintent=preview"), "{tsv}");
        assert_eq!(tsv.split('\t').count(), 13, "{tsv}");
    }

    #[test]
    fn missing_relative_access_paths_keep_protected_canonical_parent_scope() {
        let _cwd = CWD_LOCK.lock().unwrap();
        let root = temp_root("security-missing-relative-access");
        let home = root.join("home");
        let documents = home.join("Documents");
        let path = documents.join("Project").join("Plan.md");
        fs::create_dir_all(&documents).unwrap();
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(&home).unwrap();

        let absolute = absolute_access_path(Path::new("Documents/Project/Plan.md"));
        let canonical_home = fs::canonicalize(&home).unwrap();

        assert_eq!(
            protected_scope_for_home(&absolute, &canonical_home),
            ProtectedScope::Documents
        );
        assert_eq!(
            absolute,
            canonical_home
                .join("Documents")
                .join("Project")
                .join("Plan.md")
        );
        assert_eq!(
            probe_path(&absolute, AccessIntent::Read),
            AccessProbeState::Missing
        );
        assert!(!path.exists());

        std::env::set_current_dir(previous).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn full_disk_access_scope_prefers_guidance_for_denied_roots() {
        let report = SecurityScopedAccessReport {
            path: PathBuf::from("/Users/me/Library/Mail"),
            intent: AccessIntent::Index,
            scope: ProtectedScope::FullDiskAccess,
            probe: AccessProbeState::Denied,
            mode: SecurityAccessMode::FullDiskAccess,
            action: SecurityDecisionAction::Prompt,
            bookmark_required: false,
            can_read: false,
            can_write: false,
            least_privilege: false,
            reason: "protected root requires Full Disk Access guidance".to_string(),
        };

        assert_eq!(report.mode, SecurityAccessMode::FullDiskAccess);
        assert!(!report.least_privilege);
        assert!(report.as_tsv().contains("scope=full-disk-access"));
    }

    #[test]
    fn denied_mail_and_photos_scopes_require_full_disk_access_guidance_before_degradation() {
        for scope in [ProtectedScope::Mail, ProtectedScope::Photos] {
            let (mode, action, reason) =
                decide(scope, AccessProbeState::Denied, AccessIntent::Index, false);

            assert_eq!(mode, SecurityAccessMode::FullDiskAccess);
            assert_eq!(action, SecurityDecisionAction::Prompt);
            assert_eq!(reason, "protected root requires Full Disk Access guidance");
        }
    }

    #[test]
    fn worker_admission_allows_only_start_to_touch_filesystem() {
        let root = temp_root("security-worker-allow");
        let path = root.join("index.md");
        fs::write(&path, "index").unwrap();

        let admission =
            SecurityWorkerAdmissionReport::evaluate(&path, AccessIntent::Index, "index worker");

        assert_eq!(admission.worker_action, SecurityWorkerAction::Start);
        assert!(admission.can_touch_filesystem);
        assert!(!admission.refresh_on_permission_change);
        assert!(admission.as_tsv().contains("worker-action=start"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_admission_refreshes_permission_state_for_bookmark_backed_start() {
        let report = SecurityScopedAccessReport {
            path: PathBuf::from("/Users/me/Documents/Preview.md"),
            intent: AccessIntent::Preview,
            scope: ProtectedScope::Documents,
            probe: AccessProbeState::Granted,
            mode: SecurityAccessMode::SecurityScopedBookmark,
            action: SecurityDecisionAction::Allow,
            bookmark_required: true,
            can_read: true,
            can_write: false,
            least_privilege: true,
            reason: "path is readable now but should be retained with a security-scoped bookmark"
                .to_string(),
        };

        let admission = report.worker_admission("preview worker");

        assert_eq!(admission.worker_action, SecurityWorkerAction::Start);
        assert!(admission.can_touch_filesystem);
        assert!(admission.needs_bookmark_access);
        assert!(admission.refresh_on_permission_change);
        assert!(admission.as_tsv().contains("\tbookmark-access=true\t"));
        assert!(admission
            .as_tsv()
            .contains("\trefresh-on-permission-change=true\t"));
    }

    #[test]
    fn denied_protected_user_locations_prompt_for_bookmark_before_degradation() {
        let (mode, action, reason) = decide(
            ProtectedScope::Documents,
            AccessProbeState::Denied,
            AccessIntent::Preview,
            true,
        );

        assert_eq!(mode, SecurityAccessMode::SecurityScopedBookmark);
        assert_eq!(action, SecurityDecisionAction::Prompt);
        assert_eq!(
            reason,
            "access denied; request a user-selected security-scoped bookmark"
        );
    }

    #[test]
    fn worker_admission_degrades_unprotected_preview_without_filesystem_touch() {
        let report = SecurityScopedAccessReport {
            path: PathBuf::from("/Users/me/Private.md"),
            intent: AccessIntent::Preview,
            scope: ProtectedScope::None,
            probe: AccessProbeState::Denied,
            mode: SecurityAccessMode::DegradedMetadataOnly,
            action: SecurityDecisionAction::Degrade,
            bookmark_required: false,
            can_read: false,
            can_write: false,
            least_privilege: true,
            reason: "access denied; continue with metadata-only degraded mode".to_string(),
        };

        let admission = report.worker_admission("preview worker");

        assert_eq!(admission.worker_action, SecurityWorkerAction::MetadataOnly);
        assert!(!admission.can_touch_filesystem);
        assert!(!admission.needs_bookmark_access);
        assert!(admission.refresh_on_permission_change);
        assert!(admission.as_tsv().contains("worker-action=metadata-only"));
    }

    #[test]
    fn worker_admission_prompts_without_starting_full_disk_worker() {
        let report = SecurityScopedAccessReport {
            path: PathBuf::from("/Users/me/Library/Mail"),
            intent: AccessIntent::Index,
            scope: ProtectedScope::FullDiskAccess,
            probe: AccessProbeState::Denied,
            mode: SecurityAccessMode::FullDiskAccess,
            action: SecurityDecisionAction::Prompt,
            bookmark_required: false,
            can_read: false,
            can_write: false,
            least_privilege: false,
            reason: "protected root requires Full Disk Access guidance".to_string(),
        };

        let admission = report.worker_admission("index worker");

        assert_eq!(admission.worker_action, SecurityWorkerAction::Prompt);
        assert!(!admission.can_touch_filesystem);
        assert!(admission.refresh_on_permission_change);
        assert!(admission.as_tsv().contains("scope=full-disk-access"));
    }

    fn temp_root(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "gfm-{name}-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn evaluate_for_home(
        path: &Path,
        intent: AccessIntent,
        home: &Path,
    ) -> SecurityScopedAccessReport {
        let scope = protected_scope_for_home(path, home);
        let probe = probe_path(path, intent);
        let can_read = matches!(probe, AccessProbeState::Granted) && read_intent(intent);
        let can_write = matches!(probe, AccessProbeState::Granted) && write_intent(intent);
        let bookmark_required = requires_bookmark(scope, intent);
        let (mode, action, reason) = decide(scope, probe, intent, bookmark_required);
        let least_privilege = least_privilege(mode, intent);
        SecurityScopedAccessReport {
            path: path.to_path_buf(),
            intent,
            scope,
            probe,
            mode,
            action,
            bookmark_required,
            can_read,
            can_write,
            least_privilege,
            reason,
        }
    }
}
