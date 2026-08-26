use gfm_types::{GfmError, Result};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityScopedBookmark {
    pub path: PathBuf,
    pub read_only: bool,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityScopedBookmarkReport {
    pub path: PathBuf,
    pub status: SecurityScopedBookmarkStatus,
    pub read_only: bool,
    pub byte_len: usize,
    pub resolved_path: Option<PathBuf>,
    pub stale: bool,
    pub access_started: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityScopedBookmarkStatus {
    Created,
    Resolved,
    Missing,
    Unavailable,
    NotRequired,
}

impl SecurityScopedBookmarkStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Resolved => "resolved",
            Self::Missing => "missing",
            Self::Unavailable => "unavailable",
            Self::NotRequired => "not-required",
        }
    }
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

    pub fn create_bookmark(&self) -> SecurityScopedBookmarkReport {
        if !self.bookmark_required {
            return SecurityScopedBookmarkReport::not_required(self.path.clone(), false);
        }
        if self.action != SecurityDecisionAction::Allow {
            return SecurityScopedBookmarkReport::unavailable(
                self.path.clone(),
                bookmark_read_only(self.intent),
                format!(
                    "bookmark creation requires allowed access; current action is {}",
                    self.action.as_str()
                ),
            );
        }
        SecurityScopedBookmark::create(&self.path, bookmark_read_only(self.intent))
            .map(SecurityScopedBookmarkReport::created)
            .unwrap_or_else(|report| report)
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

impl SecurityScopedBookmark {
    pub fn create(
        path: impl AsRef<Path>,
        read_only: bool,
    ) -> std::result::Result<Self, SecurityScopedBookmarkReport> {
        let path = path.as_ref().to_path_buf();
        let native = gfm_mac_sys::create_security_scoped_bookmark(&path, read_only);
        match native.status {
            gfm_mac_sys::NativeBookmarkStatus::Available => Ok(Self {
                path,
                read_only,
                data: native.data,
            }),
            gfm_mac_sys::NativeBookmarkStatus::Missing => {
                Err(SecurityScopedBookmarkReport::missing(
                    path,
                    read_only,
                    native
                        .reason
                        .unwrap_or_else(|| "bookmark target missing".to_string()),
                ))
            }
            gfm_mac_sys::NativeBookmarkStatus::Unavailable => {
                Err(SecurityScopedBookmarkReport::unavailable(
                    path,
                    read_only,
                    native
                        .reason
                        .unwrap_or_else(|| "security-scoped bookmark unavailable".to_string()),
                ))
            }
        }
    }

    pub fn resolve(&self, start_access: bool) -> SecurityScopedBookmarkReport {
        let native = gfm_mac_sys::resolve_security_scoped_bookmark(&self.data, start_access);
        match native.status {
            gfm_mac_sys::NativeBookmarkStatus::Available => SecurityScopedBookmarkReport {
                path: self.path.clone(),
                status: SecurityScopedBookmarkStatus::Resolved,
                read_only: self.read_only,
                byte_len: self.data.len(),
                resolved_path: native.path,
                stale: native.stale,
                access_started: native.access_started,
                reason: None,
            },
            gfm_mac_sys::NativeBookmarkStatus::Missing => SecurityScopedBookmarkReport::missing(
                self.path.clone(),
                self.read_only,
                native
                    .reason
                    .unwrap_or_else(|| "bookmark target missing".to_string()),
            ),
            gfm_mac_sys::NativeBookmarkStatus::Unavailable => {
                SecurityScopedBookmarkReport::unavailable(
                    self.path.clone(),
                    self.read_only,
                    native.reason.unwrap_or_else(|| {
                        "security-scoped bookmark resolution unavailable".to_string()
                    }),
                )
            }
        }
    }
}

impl SecurityScopedBookmarkReport {
    fn created(bookmark: SecurityScopedBookmark) -> Self {
        Self {
            path: bookmark.path,
            status: SecurityScopedBookmarkStatus::Created,
            read_only: bookmark.read_only,
            byte_len: bookmark.data.len(),
            resolved_path: None,
            stale: false,
            access_started: false,
            reason: None,
        }
    }

    fn not_required(path: PathBuf, read_only: bool) -> Self {
        Self {
            path,
            status: SecurityScopedBookmarkStatus::NotRequired,
            read_only,
            byte_len: 0,
            resolved_path: None,
            stale: false,
            access_started: false,
            reason: Some("path does not require a retained security-scoped bookmark".to_string()),
        }
    }

    fn missing(path: PathBuf, read_only: bool, reason: String) -> Self {
        Self {
            path,
            status: SecurityScopedBookmarkStatus::Missing,
            read_only,
            byte_len: 0,
            resolved_path: None,
            stale: false,
            access_started: false,
            reason: Some(reason),
        }
    }

    fn unavailable(path: PathBuf, read_only: bool, reason: String) -> Self {
        Self {
            path,
            status: SecurityScopedBookmarkStatus::Unavailable,
            read_only,
            byte_len: 0,
            resolved_path: None,
            stale: false,
            access_started: false,
            reason: Some(reason),
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "security-bookmark\t{}\tstatus={}\tread-only={}\tbytes={}\tresolved={}\tstale={}\taccess-started={}\treason={}",
            self.path.display(),
            self.status.as_str(),
            self.read_only,
            self.byte_len,
            self.resolved_path
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.stale,
            self.access_started,
            self.reason
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string())
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
    let components = path_components(path);
    if components.iter().any(|component| component == "Volumes") {
        return ProtectedScope::ExternalVolume;
    }
    if components.iter().any(|component| component == "Network") {
        return ProtectedScope::NetworkVolume;
    }
    if components
        .windows(2)
        .any(|window| window == ["Library", "Mail"])
    {
        return ProtectedScope::FullDiskAccess;
    }
    if components.iter().any(|component| component == "Desktop") {
        ProtectedScope::Desktop
    } else if components.iter().any(|component| component == "Documents") {
        ProtectedScope::Documents
    } else if components.iter().any(|component| component == "Downloads") {
        ProtectedScope::Downloads
    } else if components
        .iter()
        .any(|component| component == "Photos Library.photoslibrary")
    {
        ProtectedScope::Photos
    } else if components
        .windows(3)
        .any(|window| window == ["Library", "Group Containers", "group.com.apple.mail"])
    {
        ProtectedScope::Mail
    } else {
        ProtectedScope::None
    }
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
        AccessIntent::Read | AccessIntent::Write | AccessIntent::Operate
    )
}

fn least_privilege(mode: SecurityAccessMode, intent: AccessIntent) -> bool {
    match mode {
        SecurityAccessMode::PlainFilesystem => true,
        SecurityAccessMode::SecurityScopedBookmark => {
            matches!(
                intent,
                AccessIntent::Read | AccessIntent::Write | AccessIntent::Operate
            )
        }
        SecurityAccessMode::DegradedMetadataOnly => {
            matches!(intent, AccessIntent::Index | AccessIntent::Preview)
        }
        SecurityAccessMode::FullDiskAccess => false,
        SecurityAccessMode::Denied => true,
    }
}

fn bookmark_read_only(intent: AccessIntent) -> bool {
    matches!(
        intent,
        AccessIntent::Read | AccessIntent::Index | AccessIntent::Preview
    )
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

fn path_components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .map(ToOwned::to_owned)
        .collect()
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
    fn protected_documents_read_requires_bookmark_retention() {
        let root = temp_root("security-documents");
        let path = root.join("Documents").join("Plan.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "plan").unwrap();

        let report = SecurityScopedAccessReport::evaluate(&path, AccessIntent::Read);

        assert_eq!(report.scope, ProtectedScope::Documents);
        assert_eq!(report.mode, SecurityAccessMode::SecurityScopedBookmark);
        assert_eq!(report.action, SecurityDecisionAction::Allow);
        assert!(report.bookmark_required);
        assert!(report.as_tsv().contains("bookmark-required=true"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn plain_paths_do_not_create_unnecessary_bookmarks() {
        let root = temp_root("security-bookmark-plain");
        let path = root.join("note.md");
        fs::write(&path, "note").unwrap();
        let report = SecurityScopedAccessReport::evaluate(&path, AccessIntent::Read);

        let bookmark = report.create_bookmark();

        assert_eq!(bookmark.status, SecurityScopedBookmarkStatus::NotRequired);
        assert_eq!(bookmark.byte_len, 0);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn protected_allowed_paths_create_and_resolve_bookmarks() {
        let root = temp_root("security-bookmark-documents");
        let path = root.join("Documents").join("Plan.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "plan").unwrap();
        let report = SecurityScopedAccessReport::evaluate(&path, AccessIntent::Read);

        let bookmark = SecurityScopedBookmark::create(&path, true).unwrap();
        let created = report.create_bookmark();
        let resolved = bookmark.resolve(false);

        assert_eq!(created.status, SecurityScopedBookmarkStatus::Created);
        assert!(created.byte_len > 0);
        assert_eq!(resolved.status, SecurityScopedBookmarkStatus::Resolved);
        assert_eq!(
            resolved
                .resolved_path
                .as_ref()
                .and_then(|path| path.canonicalize().ok()),
            Some(path.canonicalize().unwrap())
        );
        assert!(!resolved.stale);
        assert!(resolved.as_tsv().contains("status=resolved"));

        fs::remove_dir_all(root).unwrap();
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
}
