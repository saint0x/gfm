use gfm_types::{GfmError, Result};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

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
    Unknown,
}

impl AccessProbeState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Granted => "granted",
            Self::Missing => "missing",
            Self::Denied => "denied",
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

impl SecurityScopedAccessReport {
    pub fn evaluate(path: impl AsRef<Path>, intent: AccessIntent) -> Self {
        let path = path.as_ref().to_path_buf();
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

    pub fn as_tsv(&self) -> String {
        format!(
            "security-scope\t{}\tintent={}\tscope={}\tprobe={}\tmode={}\taction={}\tbookmark-required={}\tcan-read={}\tcan-write={}\tleast-privilege={}\treason={}",
            self.path.display(),
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
        AccessProbeState::Denied if scope == ProtectedScope::FullDiskAccess => (
            SecurityAccessMode::FullDiskAccess,
            SecurityDecisionAction::Prompt,
            "protected root requires Full Disk Access guidance".to_string(),
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
        AccessProbeState::Denied if bookmark_required => (
            SecurityAccessMode::SecurityScopedBookmark,
            SecurityDecisionAction::Prompt,
            "access denied; request a user-selected security-scoped bookmark".to_string(),
        ),
        AccessProbeState::Denied => (
            SecurityAccessMode::Denied,
            SecurityDecisionAction::Deny,
            "access denied and no lower-privilege fallback is valid".to_string(),
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
    std::env::var_os("HOME").map(PathBuf::from)
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
    if path.is_dir() {
        let probe = path.join(".gfm-write-probe");
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
    } else {
        match fs::OpenOptions::new().append(true).open(path) {
            Ok(_) => AccessProbeState::Granted,
            Err(err) => probe_error(err.kind()),
        }
    }
}

fn probe_error(kind: ErrorKind) -> AccessProbeState {
    match kind {
        ErrorKind::NotFound => AccessProbeState::Missing,
        ErrorKind::PermissionDenied => AccessProbeState::Denied,
        _ => AccessProbeState::Unknown,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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
