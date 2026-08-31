use crate::watch::{FileEventStream, WatchRoot};
use gfm_mac_sys::{
    copy_fileprovider_identity, copy_fileprovider_resource_values, enumerate_fileprovider_domains,
    evict_ubiquitous_item, start_downloading_ubiquitous_item, NativeFileProviderDomain,
    NativeFileProviderDomainEnumeration, NativeFileProviderDomainStatus,
    NativeFileProviderIdentity, NativeFileProviderIdentityStatus,
    NativeFileProviderOperationStatus, NativeFileProviderResourceValues,
    NativeUbiquitousDownloadingStatus,
};
use gfm_types::{FileEvent, FileEventKind, GfmError, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::path::{Path, PathBuf};

const ICLOUD_DRIVE_COMPONENT: &str = "com~apple~CloudDocs";
const MAX_PROVIDER_XATTR_VALUE_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileProviderDomain {
    ICloudDrive,
    FileProvider,
    Local,
}

impl FileProviderDomain {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ICloudDrive => "icloud-drive",
            Self::FileProvider => "fileprovider",
            Self::Local => "local",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudStorageState {
    LocalOnly,
    Downloaded,
    Evicted,
    Downloading,
    Uploading,
    Waiting,
    Conflict,
    Offline,
    Unknown,
    Removed,
}

impl CloudStorageState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalOnly => "local-only",
            Self::Downloaded => "downloaded",
            Self::Evicted => "evicted",
            Self::Downloading => "downloading",
            Self::Uploading => "uploading",
            Self::Waiting => "waiting",
            Self::Conflict => "conflict",
            Self::Offline => "offline",
            Self::Unknown => "unknown",
            Self::Removed => "removed",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "local-only" => Ok(Self::LocalOnly),
            "downloaded" => Ok(Self::Downloaded),
            "evicted" => Ok(Self::Evicted),
            "downloading" => Ok(Self::Downloading),
            "uploading" => Ok(Self::Uploading),
            "waiting" => Ok(Self::Waiting),
            "conflict" => Ok(Self::Conflict),
            "offline" => Ok(Self::Offline),
            "unknown" => Ok(Self::Unknown),
            "removed" => Ok(Self::Removed),
            other => Err(GfmError::Format(format!(
                "unsupported FileProvider storage state `{other}`"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudMaterialization {
    NotProviderBacked,
    Materialized,
    RemotePlaceholder,
    InFlight,
    Conflict,
    Offline,
    Unknown,
}

impl CloudMaterialization {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotProviderBacked => "not-provider-backed",
            Self::Materialized => "materialized",
            Self::RemotePlaceholder => "remote-placeholder",
            Self::InFlight => "in-flight",
            Self::Conflict => "conflict",
            Self::Offline => "offline",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudMaterializationSource {
    NativeUrlResource,
    NativeUrlResourceMissing,
    NativeUrlResourceUnavailable,
    NativeUrlResourceUnsupported,
    NativeFileProviderIdentityUnknown,
    NativeFileProviderIdentityMissing,
    NativeFileProviderIdentityProviderUnavailable,
    NativeFileProviderIdentityTimedOut,
    NativeFileProviderIdentityUnavailable,
    NativeFileProviderIdentityFailed,
    NativeFileProviderIdentityUnsupported,
    NativeFileProviderIdentityNoProviderForPath,
    XattrFallback,
    PathFallback,
    Filesystem,
    StateFallback,
}

impl CloudMaterializationSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NativeUrlResource => "native-url-resource",
            Self::NativeUrlResourceMissing => "native-url-resource:missing",
            Self::NativeUrlResourceUnavailable => "native-url-resource:unavailable",
            Self::NativeUrlResourceUnsupported => "native-url-resource:unsupported",
            Self::NativeFileProviderIdentityUnknown => "nsfileprovidermanager:unknown",
            Self::NativeFileProviderIdentityMissing => "nsfileprovidermanager:missing",
            Self::NativeFileProviderIdentityProviderUnavailable => {
                "nsfileprovidermanager:provider-unavailable"
            }
            Self::NativeFileProviderIdentityTimedOut => "nsfileprovidermanager:timed-out",
            Self::NativeFileProviderIdentityUnavailable => "nsfileprovidermanager:unavailable",
            Self::NativeFileProviderIdentityFailed => "nsfileprovidermanager:failed",
            Self::NativeFileProviderIdentityUnsupported => "nsfileprovidermanager:unsupported",
            Self::NativeFileProviderIdentityNoProviderForPath => {
                "nsfileprovidermanager:no-provider-for-path"
            }
            Self::XattrFallback => "xattr-fallback",
            Self::PathFallback => "path-fallback",
            Self::Filesystem => "filesystem",
            Self::StateFallback => "state-fallback",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudMaterializationConfidence {
    Native,
    ProviderIdentity,
    XattrFallback,
    PathFallback,
    Filesystem,
    StateFallback,
}

impl CloudMaterializationConfidence {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::ProviderIdentity => "provider-identity",
            Self::XattrFallback => "xattr-fallback",
            Self::PathFallback => "path-fallback",
            Self::Filesystem => "filesystem",
            Self::StateFallback => "state-fallback",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CloudBadge {
    AvailableOffline,
    Cloud,
    Downloading,
    Uploading,
    Waiting,
    Conflict,
    Offline,
}

impl CloudBadge {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AvailableOffline => "available-offline",
            Self::Cloud => "cloud",
            Self::Downloading => "downloading",
            Self::Uploading => "uploading",
            Self::Waiting => "waiting",
            Self::Conflict => "conflict",
            Self::Offline => "offline",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudCommandState {
    Enabled,
    Disabled,
    Hidden,
}

impl CloudCommandState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::Hidden => "hidden",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudCommandPolicy {
    pub download: CloudCommandState,
    pub evict: CloudCommandState,
    pub reveal_conflict: CloudCommandState,
    pub reason: Option<String>,
}

impl CloudCommandPolicy {
    fn local() -> Self {
        Self {
            download: CloudCommandState::Hidden,
            evict: CloudCommandState::Hidden,
            reveal_conflict: CloudCommandState::Hidden,
            reason: Some("not-fileprovider-backed".to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileProviderStateReport {
    pub path: PathBuf,
    pub domain: FileProviderDomain,
    pub storage_state: CloudStorageState,
    pub materialization: CloudMaterialization,
    pub materialization_source: CloudMaterializationSource,
    pub materialization_confidence: CloudMaterializationConfidence,
    pub materialization_reason: Option<String>,
    pub progress: CloudTransferProgress,
    pub badges: Vec<CloudBadge>,
    pub commands: CloudCommandPolicy,
    pub offline: bool,
    pub conflict: bool,
    pub provider_identifier: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileProviderDomainReport {
    pub path: PathBuf,
    pub domain: FileProviderDomain,
    pub native_identity_status: NativeFileProviderIdentityStatus,
    pub native_manager_status: NativeFileProviderDomainStatus,
    pub resource_status: &'static str,
    pub domain_count: usize,
    pub item_identifier: Option<String>,
    pub domain_identifier: Option<String>,
    pub matched_domain_display_name: Option<String>,
    pub matched_path_relative_to_document_storage: Option<String>,
    pub matched_domain_disconnected: Option<bool>,
    pub provider_identifier: Option<String>,
    pub source: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileProviderRegisteredDomain {
    pub domain: FileProviderDomain,
    pub identifier: Option<String>,
    pub display_name: Option<String>,
    pub path_relative_to_document_storage: Option<String>,
    pub disconnected: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileProviderDomainEnumerationReport {
    pub status: NativeFileProviderDomainStatus,
    pub domains: Vec<FileProviderRegisteredDomain>,
    pub reason: Option<String>,
}

impl FileProviderDomainEnumerationReport {
    pub fn discover() -> Self {
        let native = enumerate_fileprovider_domains();
        Self::from_native(native)
    }

    fn from_native(native: NativeFileProviderDomainEnumeration) -> Self {
        Self {
            status: native.status,
            domains: native
                .domains
                .into_iter()
                .map(|domain| {
                    let mapped = domain
                        .identifier
                        .as_deref()
                        .filter(|identifier| is_icloud_domain_identifier(identifier))
                        .map(|_| FileProviderDomain::ICloudDrive)
                        .unwrap_or(FileProviderDomain::FileProvider);
                    FileProviderRegisteredDomain {
                        domain: mapped,
                        identifier: domain.identifier,
                        display_name: domain.display_name,
                        path_relative_to_document_storage: domain.path_relative_to_document_storage,
                        disconnected: domain.disconnected,
                    }
                })
                .collect(),
            reason: native.reason,
        }
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = vec![format!(
            "fileprovider-domains\tstatus={}\tcount={}\treason={}",
            self.status.as_str(),
            self.domains.len(),
            self.reason
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
        )];
        lines.extend(self.domains.iter().map(|domain| {
            format!(
                "domain\tkind={}\tidentifier={}\tdisplay-name={}\tpath-relative={}\tdisconnected={}",
                domain.domain.as_str(),
                domain
                    .identifier
                    .as_deref()
                    .map(escape_field)
                    .unwrap_or_else(|| "-".to_string()),
                domain
                    .display_name
                    .as_deref()
                    .map(escape_field)
                    .unwrap_or_else(|| "-".to_string()),
                domain
                    .path_relative_to_document_storage
                    .as_deref()
                    .map(escape_field)
                    .unwrap_or_else(|| "-".to_string()),
                domain
                    .disconnected
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string()),
            )
        }));
        lines.join("\n")
    }
}

impl FileProviderDomainReport {
    pub fn read_path(path: impl AsRef<Path>) -> Result<Self> {
        Self::read_path_checked(path, || Ok(()))
    }

    pub fn read_path_checked(
        path: impl AsRef<Path>,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check()?;
        let path = path.as_ref().to_path_buf();
        check()?;
        ensure_fileprovider_read_path(&path)?;
        check()?;
        let hints = CloudHints::read_with_identity_checked(&path, &mut check)?;
        check()?;
        let domains = enumerate_fileprovider_domains();
        check()?;
        Ok(Self::from_hints_and_domains(path, hints, domains))
    }

    fn from_hints_and_domains(
        path: PathBuf,
        hints: CloudHints,
        domains: NativeFileProviderDomainEnumeration,
    ) -> Self {
        let domain = domain_for_path(&path, &hints);
        let matched_domain = matched_domain(&hints, &domains);
        Self {
            path,
            domain,
            native_identity_status: hints.native_identity.status,
            native_manager_status: domains.status,
            resource_status: hints.native.status.as_str(),
            domain_count: domains.domains.len(),
            item_identifier: hints.native_identity.item_identifier,
            domain_identifier: hints.native_identity.domain_identifier,
            matched_domain_display_name: matched_domain
                .and_then(|domain| domain.display_name.clone()),
            matched_path_relative_to_document_storage: matched_domain
                .and_then(|domain| domain.path_relative_to_document_storage.clone()),
            matched_domain_disconnected: matched_domain.and_then(|domain| domain.disconnected),
            provider_identifier: hints.provider_identifier,
            source: hints.source,
            reason: hints
                .native_identity
                .reason
                .or(hints.native.reason)
                .or(domains.reason),
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "fileprovider-domain\t{}\tdomain={}\tidentity-status={}\tmanager-status={}\tresource-status={}\tdomain-count={}\titem={}\tdomain-id={}\tmatched-display={}\tstorage-relative={}\tdisconnected={}\tprovider={}\tsource={}\treason={}",
            self.path.display(),
            self.domain.as_str(),
            self.native_identity_status.as_str(),
            self.native_manager_status.as_str(),
            self.resource_status,
            self.domain_count,
            self.item_identifier
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            self.domain_identifier
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            self.matched_domain_display_name
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            self.matched_path_relative_to_document_storage
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            self.matched_domain_disconnected
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.provider_identifier
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            escape_field(&self.source),
            self.reason
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudTransferDirection {
    Idle,
    Download,
    Upload,
    Materialize,
}

impl CloudTransferDirection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Download => "download",
            Self::Upload => "upload",
            Self::Materialize => "materialize",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudTransferProgress {
    pub direction: CloudTransferDirection,
    pub percent_milli: Option<u32>,
    pub requested: bool,
    pub complete: bool,
    pub indeterminate: bool,
    pub source: &'static str,
    pub reason: Option<String>,
}

impl CloudTransferProgress {
    fn idle(reason: impl Into<String>) -> Self {
        Self {
            direction: CloudTransferDirection::Idle,
            percent_milli: None,
            requested: false,
            complete: false,
            indeterminate: false,
            source: "state",
            reason: Some(reason.into()),
        }
    }

    fn complete(direction: CloudTransferDirection, reason: impl Into<String>) -> Self {
        Self {
            direction,
            percent_milli: Some(100_000),
            requested: false,
            complete: true,
            indeterminate: false,
            source: "state",
            reason: Some(reason.into()),
        }
    }

    fn from_native(
        direction: CloudTransferDirection,
        percent_milli: Option<u32>,
        requested: bool,
    ) -> Self {
        Self {
            direction,
            percent_milli,
            requested,
            complete: percent_milli == Some(100_000),
            indeterminate: percent_milli.is_none(),
            source: if percent_milli.is_some() {
                "native-url-resource"
            } else {
                "state"
            },
            reason: if percent_milli.is_some() {
                Some(
                    match direction {
                        CloudTransferDirection::Download => "native-download-progress",
                        CloudTransferDirection::Upload => "native-upload-progress",
                        CloudTransferDirection::Materialize => "native-materialize-progress",
                        CloudTransferDirection::Idle => "native-progress",
                    }
                    .to_string(),
                )
            } else {
                Some("provider-progress-unavailable".to_string())
            },
        }
    }

    fn as_tsv_fields(&self) -> String {
        format!(
            "progress-direction={}\tprogress-milli={}\tprogress-requested={}\tprogress-complete={}\tprogress-indeterminate={}\tprogress-source={}\tprogress-reason={}",
            self.direction.as_str(),
            self.percent_milli
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.requested,
            self.complete,
            self.indeterminate,
            self.source,
            self.reason
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileProviderProgressReport {
    pub path: PathBuf,
    pub state: FileProviderStateReport,
}

impl FileProviderProgressReport {
    pub fn read_path(path: impl AsRef<Path>) -> Result<Self> {
        Self::read_path_checked(path, || Ok(()))
    }

    pub fn read_path_checked(
        path: impl AsRef<Path>,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check()?;
        let path = path.as_ref().to_path_buf();
        check()?;
        let state = FileProviderStateReport::read_path_checked(&path, &mut check)?;
        check()?;
        Ok(Self { path, state })
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "fileprovider-progress\t{}\tstate={}\t{}",
            self.path.display(),
            self.state.storage_state.as_str(),
            self.state.progress.as_tsv_fields()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileProviderConflictReport {
    pub path: PathBuf,
    pub state: FileProviderStateReport,
    pub has_unresolved_conflict: bool,
    pub affected_paths: Vec<PathBuf>,
    pub reveal_command: CloudCommandState,
    pub block_operations: bool,
    pub reason: String,
}

impl FileProviderConflictReport {
    pub fn read_path(path: impl AsRef<Path>) -> Result<Self> {
        Self::read_path_checked(path, || Ok(()))
    }

    pub fn read_path_checked(
        path: impl AsRef<Path>,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check()?;
        let path = path.as_ref().to_path_buf();
        check()?;
        let state = FileProviderStateReport::read_path_checked(&path, &mut check)?;
        check()?;
        let has_unresolved_conflict = state.storage_state == CloudStorageState::Conflict;
        let affected_paths = if has_unresolved_conflict {
            vec![path.clone()]
        } else {
            Vec::new()
        };
        check()?;
        let reason = if has_unresolved_conflict {
            "conflict-requires-user-resolution"
        } else if state.domain == FileProviderDomain::Local {
            "not-fileprovider-backed"
        } else {
            "no-provider-conflict"
        };

        Ok(Self {
            path,
            reveal_command: state.commands.reveal_conflict,
            block_operations: has_unresolved_conflict,
            state,
            has_unresolved_conflict,
            affected_paths,
            reason: reason.to_string(),
        })
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "fileprovider-conflict\t{}\tconflict={}\tstate={}\taffected={}\taffected-paths={}\treveal={}\tblock-operations={}\treason={}",
            self.path.display(),
            self.has_unresolved_conflict,
            self.state.storage_state.as_str(),
            self.affected_paths.len(),
            affected_paths_field(&self.affected_paths),
            self.reveal_command.as_str(),
            self.block_operations,
            escape_field(&self.reason),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileProviderOperation {
    Download,
    Evict,
}

impl FileProviderOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Download => "download",
            Self::Evict => "evict",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "download" => Ok(Self::Download),
            "evict" => Ok(Self::Evict),
            other => Err(GfmError::Format(format!(
                "unsupported fileprovider operation `{other}`"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileProviderOperationDisposition {
    Completed,
    Refused,
    Denied,
    Missing,
    Unsupported,
    Unavailable,
    Cancelled,
    Failed,
}

impl FileProviderOperationDisposition {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Refused => "refused",
            Self::Denied => "denied",
            Self::Missing => "missing",
            Self::Unsupported => "unsupported",
            Self::Unavailable => "unavailable",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileProviderOperationReport {
    pub path: PathBuf,
    pub operation: FileProviderOperation,
    pub disposition: FileProviderOperationDisposition,
    pub before: FileProviderStateReport,
    pub after: Option<FileProviderStateReport>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileProviderInvalidationReport {
    pub path: PathBuf,
    pub previous: CloudStorageState,
    pub current: FileProviderStateReport,
    pub state_changed: bool,
    pub invalidate_icon: bool,
    pub invalidate_preview_memory: bool,
    pub invalidate_preview_disk: bool,
    pub invalidate_sidebar: bool,
    pub reindex_metadata: bool,
    pub reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileProviderStateSnapshot {
    pub entries: Vec<FileProviderStateSnapshotEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileProviderStateSnapshotEntry {
    pub path: PathBuf,
    pub state: CloudStorageState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileProviderStateInvalidationReport {
    pub initialized: bool,
    pub changes: Vec<FileProviderInvalidationReport>,
    pub invalidate_icon: bool,
    pub invalidate_preview_memory: bool,
    pub invalidate_preview_disk: bool,
    pub invalidate_sidebar: bool,
    pub reindex_metadata: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FileProviderObservedEventKind {
    Create,
    Metadata,
    Modify,
    Remove,
    Rename,
    Rescan,
    Other,
}

impl FileProviderObservedEventKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Metadata => "metadata",
            Self::Modify => "modify",
            Self::Remove => "remove",
            Self::Rename => "rename",
            Self::Rescan => "rescan",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileProviderObservedInvalidation {
    pub events: usize,
    pub event_kinds: Vec<FileProviderObservedEventKind>,
    pub paths: Vec<PathBuf>,
    pub report: FileProviderStateInvalidationReport,
}

pub struct FileProviderStateObserver {
    stream: FileEventStream,
    snapshot: FileProviderStateSnapshot,
}

struct FileProviderSnapshotLookup<'a> {
    snapshot: Option<&'a FileProviderStateSnapshot>,
    states: BTreeMap<&'a Path, CloudStorageState>,
}

impl<'a> FileProviderSnapshotLookup<'a> {
    fn new(snapshot: Option<&'a FileProviderStateSnapshot>) -> Self {
        Self {
            snapshot,
            states: snapshot
                .map(FileProviderStateSnapshot::state_index)
                .unwrap_or_default(),
        }
    }

    fn contains_path(&self, path: &Path) -> bool {
        self.states.contains_key(path)
    }

    fn contains_path_or_descendant(&self, path: &Path) -> bool {
        self.contains_path(path)
            || self.snapshot.is_some_and(|snapshot| {
                snapshot
                    .entries
                    .iter()
                    .any(|entry| entry.path.starts_with(path))
            })
    }

    fn tracked_descendants_of(&self, root: &Path) -> Vec<PathBuf> {
        self.snapshot
            .map(|snapshot| snapshot.tracked_descendants_of(root))
            .unwrap_or_default()
    }
}

impl FileProviderInvalidationReport {
    pub fn evaluate(
        path: impl AsRef<Path>,
        previous: CloudStorageState,
    ) -> Result<FileProviderInvalidationReport> {
        Self::evaluate_checked(path, previous, || Ok(()))
    }

    pub fn evaluate_checked(
        path: impl AsRef<Path>,
        previous: CloudStorageState,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<FileProviderInvalidationReport> {
        check()?;
        let path = path.as_ref().to_path_buf();
        check()?;
        let current = FileProviderStateReport::read_path_checked(&path, &mut check)?;
        check()?;
        Ok(Self::from_current(path, previous, current))
    }

    fn from_current(
        path: PathBuf,
        previous: CloudStorageState,
        current: FileProviderStateReport,
    ) -> FileProviderInvalidationReport {
        let state_changed = previous != current.storage_state;
        let provider_visible = current.domain != FileProviderDomain::Local
            || previous != CloudStorageState::LocalOnly
            || !current.badges.is_empty();
        let invalidate_sidebar = provider_visible
            && (state_changed
                || matches!(
                    current.storage_state,
                    CloudStorageState::Downloaded
                        | CloudStorageState::Evicted
                        | CloudStorageState::Downloading
                        | CloudStorageState::Uploading
                        | CloudStorageState::Waiting
                        | CloudStorageState::Conflict
                        | CloudStorageState::Offline
                        | CloudStorageState::Unknown
                        | CloudStorageState::Removed
                ));
        let reason = if !provider_visible {
            "not-provider-visible"
        } else if state_changed {
            "fileprovider-state-changed"
        } else {
            "fileprovider-state-unchanged"
        };

        FileProviderInvalidationReport {
            path,
            previous,
            state_changed,
            invalidate_icon: provider_visible && state_changed,
            invalidate_preview_memory: provider_visible && state_changed,
            invalidate_preview_disk: provider_visible && state_changed,
            invalidate_sidebar,
            reindex_metadata: provider_visible && state_changed,
            current,
            reason,
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "fileprovider-invalidation\t{}\tprevious={}\tcurrent={}\tchanged={}\ticon={}\tpreview-memory={}\tpreview-disk={}\tsidebar={}\treindex-metadata={}\treason={}",
            self.path.display(),
            self.previous.as_str(),
            self.current.storage_state.as_str(),
            self.state_changed,
            self.invalidate_icon,
            self.invalidate_preview_memory,
            self.invalidate_preview_disk,
            self.invalidate_sidebar,
            self.reindex_metadata,
            self.reason
        )
    }
}

impl FileProviderStateSnapshot {
    pub fn from_paths(paths: impl IntoIterator<Item = PathBuf>) -> Result<Self> {
        let mut entries = Vec::new();
        let mut seen_paths = BTreeSet::new();
        for path in paths {
            if !seen_paths.insert(path.clone()) {
                continue;
            }
            let report = FileProviderStateReport::read_path(&path)?;
            if should_persist_fileprovider_snapshot_entry(&report) {
                entries.push(FileProviderStateSnapshotEntry {
                    path,
                    state: report.storage_state,
                });
            }
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        entries.dedup_by(|left, right| left.path == right.path);
        Ok(Self { entries })
    }

    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        Self::read_checked(path, || Ok(()))
    }

    pub fn read_checked(
        path: impl AsRef<Path>,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        let path = path.as_ref();
        check_control()?;
        let file = File::open(path).map_err(|err| GfmError::io(path, err))?;
        check_control()?;
        let mut lines = BufReader::new(file).lines();
        let header = lines
            .next()
            .transpose()
            .map_err(|err| GfmError::io(path, err))?;
        check_control()?;
        match header.as_deref() {
            Some("gfm-fileprovider-state-v1") => {}
            Some(other) => {
                return Err(GfmError::Format(format!(
                    "unsupported FileProvider state header `{other}` in {}",
                    path.display()
                )))
            }
            None => {
                return Err(GfmError::Format(format!(
                    "empty FileProvider state file {}",
                    path.display()
                )))
            }
        }
        let mut entries = Vec::new();
        let mut seen_paths = BTreeSet::new();
        for (line_index, line) in lines.enumerate() {
            check_control()?;
            let line = line.map_err(|err| GfmError::io(path, err))?;
            if line.trim().is_empty() {
                continue;
            }
            let fields = line.split('\t').collect::<Vec<_>>();
            if fields.len() != 2 {
                return Err(GfmError::Format(format!(
                    "{}:{} expected 2 tab-separated fields: state, path",
                    path.display(),
                    line_index + 2
                )));
            }
            let state = CloudStorageState::parse(fields[0]).map_err(|err| {
                GfmError::Format(format!(
                    "{}:{} invalid FileProvider state `{}`: {err}",
                    path.display(),
                    line_index + 2,
                    fields[0]
                ))
            })?;
            let entry_path = PathBuf::from(unescape_field(fields[1]));
            if !seen_paths.insert(entry_path.clone()) {
                return Err(GfmError::Format(format!(
                    "{}:{} duplicate FileProvider state path `{}`",
                    path.display(),
                    line_index + 2,
                    entry_path.display()
                )));
            }
            entries.push(FileProviderStateSnapshotEntry {
                state,
                path: entry_path,
            });
        }
        Ok(Self { entries })
    }

    pub fn write(&self, path: impl AsRef<Path>) -> Result<()> {
        self.write_checked(path, || Ok(()))
    }

    pub fn write_checked(
        &self,
        path: impl AsRef<Path>,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<()> {
        let path = path.as_ref();
        validate_unique_fileprovider_snapshot_paths(path, &self.entries)?;
        let mut entries = self.entries.clone();
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        let mut output = String::from("gfm-fileprovider-state-v1\n");
        for entry in &entries {
            output.push_str(&format!(
                "{}\t{}\n",
                entry.state.as_str(),
                escape_field(&entry.path.to_string_lossy())
            ));
        }
        atomic_write_text_checked(path, &output, &mut check_control)
    }

    fn state_index(&self) -> BTreeMap<&Path, CloudStorageState> {
        self.entries
            .iter()
            .map(|entry| (entry.path.as_path(), entry.state))
            .collect()
    }

    fn tracked_descendants_of(&self, root: &Path) -> Vec<PathBuf> {
        self.entries
            .iter()
            .filter(|entry| entry.path != root && entry.path.starts_with(root))
            .map(|entry| entry.path.clone())
            .collect()
    }
}

fn validate_unique_fileprovider_snapshot_paths(
    path: &Path,
    entries: &[FileProviderStateSnapshotEntry],
) -> Result<()> {
    let mut seen_paths = BTreeSet::new();
    for entry in entries {
        if !seen_paths.insert(entry.path.clone()) {
            return Err(GfmError::Format(format!(
                "duplicate FileProvider state path `{}` before writing {}",
                entry.path.display(),
                path.display()
            )));
        }
    }
    Ok(())
}

impl FileProviderStateInvalidationReport {
    pub fn evaluate(
        previous: Option<&FileProviderStateSnapshot>,
        current_paths: impl IntoIterator<Item = PathBuf>,
    ) -> Result<(Self, FileProviderStateSnapshot)> {
        Self::evaluate_checked(previous, current_paths, || Ok(()))
    }

    pub fn evaluate_checked(
        previous: Option<&FileProviderStateSnapshot>,
        current_paths: impl IntoIterator<Item = PathBuf>,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<(Self, FileProviderStateSnapshot)> {
        check()?;
        let initialized = previous.is_none();
        let previous_states = previous
            .map(FileProviderStateSnapshot::state_index)
            .unwrap_or_default();
        let mut changes = Vec::new();
        let mut current_entries = Vec::new();
        let mut seen_current_paths = BTreeSet::new();
        for path in current_paths {
            check()?;
            if !seen_current_paths.insert(path.clone()) {
                continue;
            }
            check()?;
            let previous_state = previous_states.get(path.as_path()).copied();
            let path_exists = path
                .try_exists()
                .map_err(|err| GfmError::io(&path, format!("path existence unavailable: {err}")))?;
            check()?;
            let current = if path_exists
                || (previous_state.is_none() && is_evicted_placeholder_path(&path))
            {
                Some(FileProviderStateReport::read_path_checked(
                    &path, &mut check,
                )?)
            } else if previous_state.is_some() {
                Some(FileProviderStateReport::removed(path.clone()))
            } else {
                None
            };
            check()?;
            let current = current.ok_or_else(|| GfmError::io(&path, "path does not exist"))?;
            let previous_state = previous_state.unwrap_or(CloudStorageState::LocalOnly);
            let change =
                FileProviderInvalidationReport::from_current(path.clone(), previous_state, current);
            if change.current.storage_state != CloudStorageState::Removed
                && should_persist_fileprovider_snapshot_entry(&change.current)
            {
                current_entries.push(FileProviderStateSnapshotEntry {
                    path,
                    state: change.current.storage_state,
                });
            }
            if (initialized || change.state_changed)
                && fileprovider_invalidation_change_is_visible(&change)
            {
                changes.push(change);
            }
        }
        let snapshot = FileProviderStateSnapshot {
            entries: current_entries,
        };
        Ok((Self::from_changes(initialized, changes), snapshot))
    }

    fn from_changes(initialized: bool, changes: Vec<FileProviderInvalidationReport>) -> Self {
        Self {
            initialized,
            invalidate_icon: changes.iter().any(|change| change.invalidate_icon),
            invalidate_preview_memory: changes
                .iter()
                .any(|change| change.invalidate_preview_memory),
            invalidate_preview_disk: changes.iter().any(|change| change.invalidate_preview_disk),
            invalidate_sidebar: changes.iter().any(|change| change.invalidate_sidebar),
            reindex_metadata: changes.iter().any(|change| change.reindex_metadata),
            changes,
        }
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = vec![format!(
            "fileprovider-state-invalidation\tinitialized={}\tchanged={}\ticon={}\tpreview-memory={}\tpreview-disk={}\tsidebar={}\treindex-metadata={}",
            self.initialized,
            self.changes.len(),
            self.invalidate_icon,
            self.invalidate_preview_memory,
            self.invalidate_preview_disk,
            self.invalidate_sidebar,
            self.reindex_metadata
        )];
        lines.extend(
            self.changes
                .iter()
                .map(FileProviderInvalidationReport::as_tsv),
        );
        lines.join("\n")
    }
}

fn should_persist_fileprovider_snapshot_entry(report: &FileProviderStateReport) -> bool {
    report.domain != FileProviderDomain::Local
        || report.storage_state != CloudStorageState::LocalOnly
        || !report.badges.is_empty()
}

fn fileprovider_invalidation_change_is_visible(change: &FileProviderInvalidationReport) -> bool {
    change.reason != "not-provider-visible"
}

impl FileProviderObservedInvalidation {
    pub fn evaluate(
        previous: Option<&FileProviderStateSnapshot>,
        events: impl IntoIterator<Item = FileEvent>,
    ) -> Result<(Self, FileProviderStateSnapshot)> {
        Self::evaluate_checked(previous, events, || Ok(()))
    }

    pub fn evaluate_checked(
        previous: Option<&FileProviderStateSnapshot>,
        events: impl IntoIterator<Item = FileEvent>,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<(Self, FileProviderStateSnapshot)> {
        check()?;
        let mut event_count = 0;
        let mut event_kinds = BTreeSet::new();
        let mut paths = BTreeSet::new();
        let previous_lookup = FileProviderSnapshotLookup::new(previous);
        for event in events {
            check()?;
            event_count += 1;
            event_kinds.insert(fileprovider_observed_event_kind(&event.kind));
            for path in paths_for_fileprovider_event_checked(&previous_lookup, &event, &mut check)?
            {
                paths.insert(path);
            }
        }
        check()?;
        let event_kinds = event_kinds.into_iter().collect::<Vec<_>>();
        let paths = paths.into_iter().collect::<Vec<_>>();
        let (report, snapshot) = if paths.is_empty() {
            let snapshot = previous
                .cloned()
                .unwrap_or_else(|| FileProviderStateSnapshot {
                    entries: Vec::new(),
                });
            (
                FileProviderStateInvalidationReport::from_changes(previous.is_none(), Vec::new()),
                snapshot,
            )
        } else {
            let (report, event_snapshot) = FileProviderStateInvalidationReport::evaluate_checked(
                previous,
                paths.clone(),
                &mut check,
            )?;
            let snapshot = merge_observed_snapshot(previous, &paths, event_snapshot)?;
            (report, snapshot)
        };
        check()?;
        Ok((
            Self {
                events: event_count,
                event_kinds,
                paths,
                report,
            },
            snapshot,
        ))
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = vec![format!(
            "fileprovider-observed-invalidation\tevents={}\tevent-kinds={}\tpaths={}",
            self.events,
            self.event_kinds
                .iter()
                .map(|kind| kind.as_str())
                .collect::<Vec<_>>()
                .join(","),
            self.paths.len()
        )];
        lines.push(self.report.as_tsv());
        lines.join("\n")
    }
}

fn merge_observed_snapshot(
    previous: Option<&FileProviderStateSnapshot>,
    observed_paths: &[PathBuf],
    event_snapshot: FileProviderStateSnapshot,
) -> Result<FileProviderStateSnapshot> {
    let observed_exact = observed_paths
        .iter()
        .map(PathBuf::as_path)
        .collect::<BTreeSet<_>>();
    let mut entries = previous
        .map(|snapshot| snapshot.entries.clone())
        .unwrap_or_default();
    entries.retain(|entry| {
        !observed_exact.contains(entry.path.as_path())
            && !observed_paths
                .iter()
                .any(|path| entry.path.starts_with(path))
    });
    entries.extend(event_snapshot.entries);
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    validate_unique_fileprovider_snapshot_paths(
        Path::new("merged observed FileProvider snapshot"),
        &entries,
    )?;
    Ok(FileProviderStateSnapshot { entries })
}

impl FileProviderStateObserver {
    pub fn watch(roots: &[WatchRoot], snapshot: Option<FileProviderStateSnapshot>) -> Result<Self> {
        Ok(Self {
            stream: FileEventStream::watch(roots)?,
            snapshot: snapshot.unwrap_or_else(|| FileProviderStateSnapshot {
                entries: Vec::new(),
            }),
        })
    }

    pub fn observe_once(&mut self) -> Result<FileProviderObservedInvalidation> {
        let event = self.stream.recv()?;
        self.apply_events([event])
    }

    pub fn drain_available(
        &mut self,
        max_events: usize,
    ) -> Result<Option<FileProviderObservedInvalidation>> {
        self.drain_available_checked(max_events, || Ok(()))
    }

    pub fn drain_available_checked(
        &mut self,
        max_events: usize,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<Option<FileProviderObservedInvalidation>> {
        let mut events = Vec::new();
        for _ in 0..max_events {
            check()?;
            match self.stream.try_recv() {
                Some(Ok(event)) => events.push(event),
                Some(Err(err)) => return Err(err),
                None => break,
            }
        }
        check()?;
        if events.is_empty() {
            return Ok(None);
        }
        self.apply_events_checked(events, check).map(Some)
    }

    pub fn snapshot(&self) -> &FileProviderStateSnapshot {
        &self.snapshot
    }

    fn apply_events(
        &mut self,
        events: impl IntoIterator<Item = FileEvent>,
    ) -> Result<FileProviderObservedInvalidation> {
        self.apply_events_checked(events, || Ok(()))
    }

    fn apply_events_checked(
        &mut self,
        events: impl IntoIterator<Item = FileEvent>,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<FileProviderObservedInvalidation> {
        let (observed, snapshot) = FileProviderObservedInvalidation::evaluate_checked(
            Some(&self.snapshot),
            events,
            &mut check,
        )?;
        check()?;
        self.snapshot = snapshot;
        Ok(observed)
    }
}

fn fileprovider_observed_event_kind(kind: &FileEventKind) -> FileProviderObservedEventKind {
    match kind {
        FileEventKind::Create => FileProviderObservedEventKind::Create,
        FileEventKind::Metadata => FileProviderObservedEventKind::Metadata,
        FileEventKind::Modify => FileProviderObservedEventKind::Modify,
        FileEventKind::Remove => FileProviderObservedEventKind::Remove,
        FileEventKind::Rename { .. } => FileProviderObservedEventKind::Rename,
        FileEventKind::Rescan => FileProviderObservedEventKind::Rescan,
        FileEventKind::Other => FileProviderObservedEventKind::Other,
    }
}

fn paths_for_fileprovider_event_checked(
    previous: &FileProviderSnapshotLookup<'_>,
    event: &FileEvent,
    mut check: impl FnMut() -> Result<()>,
) -> Result<Vec<PathBuf>> {
    check()?;
    match &event.kind {
        FileEventKind::Rename { from, to } => {
            let mut paths = BTreeSet::new();
            paths.extend(observed_fileprovider_paths_for_root_checked(
                previous, from, &mut check,
            )?);
            if fileprovider_observed_path_exists_checked(to, &mut check)?
                && previous.contains_path_or_descendant(from)
            {
                paths.insert(to.clone());
            }
            check()?;
            paths.extend(observed_fileprovider_paths_for_root_checked(
                previous, to, &mut check,
            )?);
            paths.extend(remapped_tracked_fileprovider_paths_checked(
                previous, from, to, &mut check,
            )?);
            Ok(paths.into_iter().collect())
        }
        FileEventKind::Remove => {
            observed_fileprovider_paths_for_root_checked(previous, &event.path, &mut check)
        }
        FileEventKind::Create
        | FileEventKind::Metadata
        | FileEventKind::Modify
        | FileEventKind::Rescan
        | FileEventKind::Other => {
            if should_read_observed_fileprovider_path_checked(previous, &event.path, &mut check)? {
                Ok(vec![event.path.clone()])
            } else {
                Ok(Vec::new())
            }
        }
    }
}

fn observed_fileprovider_paths_for_root_checked(
    previous: &FileProviderSnapshotLookup<'_>,
    root: &Path,
    mut check: impl FnMut() -> Result<()>,
) -> Result<Vec<PathBuf>> {
    check()?;
    let mut paths = BTreeSet::new();
    if should_read_observed_fileprovider_path_checked(previous, root, &mut check)? {
        paths.insert(root.to_path_buf());
    }
    check()?;
    paths.extend(previous.tracked_descendants_of(root));
    Ok(paths.into_iter().collect())
}

fn remapped_tracked_fileprovider_paths_checked(
    previous: &FileProviderSnapshotLookup<'_>,
    from: &Path,
    to: &Path,
    mut check: impl FnMut() -> Result<()>,
) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for path in previous.tracked_descendants_of(from) {
        check()?;
        let Some(path) = path.strip_prefix(from).ok().map(|suffix| to.join(suffix)) else {
            continue;
        };
        if should_read_observed_fileprovider_path_checked(previous, &path, &mut check)? {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn should_read_observed_fileprovider_path_checked(
    previous: &FileProviderSnapshotLookup<'_>,
    path: &Path,
    mut check: impl FnMut() -> Result<()>,
) -> Result<bool> {
    check()?;
    if previous.contains_path(path) {
        return Ok(true);
    }
    if !fileprovider_observed_path_exists_checked(path, &mut check)? {
        return Ok(false);
    }
    is_observable_fileprovider_path_checked(previous, path, check)
}

fn is_observable_fileprovider_path_checked(
    previous: &FileProviderSnapshotLookup<'_>,
    path: &Path,
    mut check: impl FnMut() -> Result<()>,
) -> Result<bool> {
    check()?;
    if previous.contains_path(path) {
        return Ok(true);
    }
    if !fileprovider_observed_path_exists_checked(path, &mut check)? {
        return Ok(false);
    }
    check()?;
    let hints = CloudHints::read_checked(path, &mut check)?;
    check()?;
    Ok(strong_provider_path_hint(path)
        && !native_proves_local_only(&hints)
        && !weak_path_hint_without_provider_evidence(&hints)
        || observable_fileprovider_path_from_hints(path, &hints))
}

fn fileprovider_observed_path_exists_checked(
    path: &Path,
    mut check: impl FnMut() -> Result<()>,
) -> Result<bool> {
    check()?;
    path.try_exists()
        .map_err(|err| GfmError::io(path, format!("observed path existence unavailable: {err}")))
}

fn observable_fileprovider_path_from_hints(path: &Path, hints: &CloudHints) -> bool {
    if native_proves_local_only(hints) || weak_path_hint_without_provider_evidence(hints) {
        return false;
    }
    hints.source != "filesystem" || domain_for_path(path, hints) != FileProviderDomain::Local
}

fn weak_path_hint_without_provider_evidence(hints: &CloudHints) -> bool {
    path_only_provider_hint(&hints.source)
        && hints.xattrs.is_empty()
        && !native_has_fileprovider_values(&hints.native)
        && !native_provider_state_unavailable(hints)
        && hints.native_identity.status != NativeFileProviderIdentityStatus::Available
}

fn strong_provider_path_hint(path: &Path) -> bool {
    path_components(path)
        .iter()
        .any(|component| component == ICLOUD_DRIVE_COMPONENT)
        || is_evicted_placeholder_path(path)
}

impl FileProviderOperationReport {
    pub fn execute(path: impl AsRef<Path>, operation: FileProviderOperation) -> Result<Self> {
        Self::execute_checked(path, operation, || Ok(()))
    }

    pub fn execute_checked(
        path: impl AsRef<Path>,
        operation: FileProviderOperation,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check_control()?;
        let path = path.as_ref().to_path_buf();
        check_control()?;
        let before = FileProviderStateReport::read_path_checked(&path, &mut check_control)?;
        check_control()?;
        let command = match operation {
            FileProviderOperation::Download => before.commands.download,
            FileProviderOperation::Evict => before.commands.evict,
        };
        check_control()?;
        if path.try_exists().ok() == Some(false) {
            return Ok(Self::missing(path, operation, before));
        }
        check_control()?;
        if before.storage_state == CloudStorageState::Conflict {
            return Ok(Self::refused(
                path,
                operation,
                before,
                "provider-conflict-requires-resolution",
            ));
        }
        check_control()?;
        if before.commands.reason.as_deref() == Some("not-native-provider-backed") {
            return Ok(Self::refused(
                path,
                operation,
                before,
                "not-native-provider-backed",
            ));
        }
        check_control()?;
        if command != CloudCommandState::Enabled {
            return Ok(Self::refused(
                path,
                operation,
                before,
                "operation-disabled-for-current-state",
            ));
        }
        check_control()?;
        if before.domain == FileProviderDomain::Local || !before.source_contains_native_resource() {
            return Ok(Self::refused(
                path,
                operation,
                before,
                "not-native-provider-backed",
            ));
        }

        check_control()?;
        let result = match operation {
            FileProviderOperation::Download => start_downloading_ubiquitous_item(&path),
            FileProviderOperation::Evict => evict_ubiquitous_item(&path),
        };
        match result.status {
            NativeFileProviderOperationStatus::Completed => {
                let after =
                    FileProviderStateReport::read_path_checked(&path, &mut check_control).ok();
                Ok(Self {
                    path,
                    operation,
                    disposition: FileProviderOperationDisposition::Completed,
                    before,
                    after,
                    reason: None,
                })
            }
            NativeFileProviderOperationStatus::Missing
            | NativeFileProviderOperationStatus::PermissionDenied
            | NativeFileProviderOperationStatus::Unavailable
            | NativeFileProviderOperationStatus::Cancelled
            | NativeFileProviderOperationStatus::UnsupportedPath
            | NativeFileProviderOperationStatus::Failed => Ok(Self {
                path,
                operation,
                disposition: disposition_for_native_fileprovider_operation(result.status),
                before,
                after: None,
                reason: result.reason,
            }),
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "fileprovider-operation\t{}\toperation={}\tdisposition={}\tbefore-state={}\tafter-state={}\treason={}",
            self.path.display(),
            self.operation.as_str(),
            self.disposition.as_str(),
            self.before.storage_state.as_str(),
            self.after
                .as_ref()
                .map(|report| report.storage_state.as_str())
                .unwrap_or("-"),
            self.reason
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
        )
    }

    fn refused(
        path: PathBuf,
        operation: FileProviderOperation,
        before: FileProviderStateReport,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            path,
            operation,
            disposition: FileProviderOperationDisposition::Refused,
            before,
            after: None,
            reason: Some(reason.into()),
        }
    }

    fn missing(
        path: PathBuf,
        operation: FileProviderOperation,
        mut before: FileProviderStateReport,
    ) -> Self {
        if before.storage_state == CloudStorageState::Unknown && is_evicted_placeholder_path(&path)
        {
            before.storage_state = CloudStorageState::Evicted;
            before.materialization = CloudMaterialization::RemotePlaceholder;
            before.materialization_source = CloudMaterializationSource::PathFallback;
            before.materialization_confidence = CloudMaterializationConfidence::PathFallback;
            before.materialization_reason =
                Some("fileprovider-path-missing-placeholder".to_string());
        }
        Self {
            path,
            operation,
            disposition: FileProviderOperationDisposition::Missing,
            before,
            after: None,
            reason: Some("fileprovider-path-missing".to_string()),
        }
    }
}

fn disposition_for_native_fileprovider_operation(
    status: NativeFileProviderOperationStatus,
) -> FileProviderOperationDisposition {
    match status {
        NativeFileProviderOperationStatus::Completed => FileProviderOperationDisposition::Completed,
        NativeFileProviderOperationStatus::PermissionDenied => {
            FileProviderOperationDisposition::Denied
        }
        NativeFileProviderOperationStatus::Unavailable => {
            FileProviderOperationDisposition::Unavailable
        }
        NativeFileProviderOperationStatus::Cancelled => FileProviderOperationDisposition::Cancelled,
        NativeFileProviderOperationStatus::Missing => FileProviderOperationDisposition::Missing,
        NativeFileProviderOperationStatus::UnsupportedPath => {
            FileProviderOperationDisposition::Unsupported
        }
        NativeFileProviderOperationStatus::Failed => FileProviderOperationDisposition::Failed,
    }
}

impl FileProviderStateReport {
    pub fn read_path(path: impl AsRef<Path>) -> Result<Self> {
        Self::read_path_checked(path, || Ok(()))
    }

    pub fn read_path_checked(
        path: impl AsRef<Path>,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check()?;
        let path = path.as_ref().to_path_buf();
        check()?;
        ensure_fileprovider_read_path(&path)?;
        check()?;
        Self::from_path_checked(path, check)
    }

    pub fn from_path(path: PathBuf) -> Self {
        let hints = CloudHints::read(&path);
        Self::from_hints(path, hints)
    }

    pub fn from_path_checked(path: PathBuf, mut check: impl FnMut() -> Result<()>) -> Result<Self> {
        check()?;
        let hints = CloudHints::read_checked(&path, &mut check)?;
        check()?;
        Ok(Self::from_hints(path, hints))
    }

    pub fn from_path_with_native_identity(path: PathBuf) -> Self {
        let hints = CloudHints::read_with_identity(&path);
        Self::from_hints(path, hints)
    }

    pub fn from_path_with_native_identity_checked(
        path: PathBuf,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check()?;
        let hints = CloudHints::read_with_identity_checked(&path, &mut check)?;
        check()?;
        Ok(Self::from_hints(path, hints))
    }

    fn from_hints(path: PathBuf, hints: CloudHints) -> Self {
        let domain = domain_for_path(&path, &hints);
        let storage_state = storage_state_for_path(&path, domain, &hints);
        let materialization = materialization_for_state(storage_state);
        let materialization_source = materialization_source_for_state(storage_state, &hints);
        let materialization_confidence =
            materialization_confidence_for_source(materialization_source);
        let materialization_reason = materialization_reason_for_state(storage_state, &hints);
        let progress = progress_for_state(storage_state, &hints);
        let mut badges = badges_for_state(storage_state);
        badges.sort();
        badges.dedup();
        let commands = command_policy(domain, storage_state, provider_commands_available(&hints));

        Self {
            path,
            domain,
            storage_state,
            materialization,
            materialization_source,
            materialization_confidence,
            materialization_reason,
            progress,
            badges,
            commands,
            offline: matches!(
                storage_state,
                CloudStorageState::Offline | CloudStorageState::Evicted
            ),
            conflict: storage_state == CloudStorageState::Conflict,
            provider_identifier: hints.provider_identifier,
            source: hints.source,
        }
    }

    fn removed(path: PathBuf) -> Self {
        let storage_state = CloudStorageState::Removed;
        let commands = CloudCommandPolicy::local();
        Self {
            domain: removed_provider_domain_for_path(&path),
            path,
            storage_state,
            materialization: CloudMaterialization::Unknown,
            materialization_source: CloudMaterializationSource::StateFallback,
            materialization_confidence: CloudMaterializationConfidence::StateFallback,
            materialization_reason: Some("fileprovider-item-removed".to_string()),
            progress: CloudTransferProgress::idle("fileprovider-item-removed"),
            badges: Vec::new(),
            commands,
            offline: true,
            conflict: false,
            provider_identifier: None,
            source: "removed".to_string(),
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "fileprovider-state\t{}\tdomain={}\tstate={}\tmaterialization={}\tmaterialization-source={}\tmaterialization-confidence={}\tmaterialization-reason={}\toffline={}\tconflict={}\tbadges={}\t{}\tdownload={}\tevict={}\treveal-conflict={}\tprovider={}\tsource={}\treason={}",
            self.path.display(),
            self.domain.as_str(),
            self.storage_state.as_str(),
            self.materialization.as_str(),
            self.materialization_source.as_str(),
            self.materialization_confidence.as_str(),
            self.materialization_reason
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            self.offline,
            self.conflict,
            self.badges
                .iter()
                .map(|badge| badge.as_str())
                .collect::<Vec<_>>()
                .join(","),
            self.progress.as_tsv_fields(),
            self.commands.download.as_str(),
            self.commands.evict.as_str(),
            self.commands.reveal_conflict.as_str(),
            self.provider_identifier
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            escape_field(&self.source),
            self.commands
                .reason
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
        )
    }

    fn source_contains_native_resource(&self) -> bool {
        self.source
            .split('+')
            .any(|source| source == "native-url-resource")
    }
}

fn ensure_fileprovider_read_path(path: &Path) -> Result<()> {
    match path.try_exists() {
        Ok(true) => Ok(()),
        Ok(false) if is_evicted_placeholder_path(path) => Ok(()),
        Ok(false) => Err(GfmError::io(path, "path does not exist")),
        Err(err) => Err(GfmError::io(
            path,
            format!("path existence unavailable: {err}"),
        )),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CloudHints {
    native: NativeFileProviderResourceValues,
    native_identity: NativeFileProviderIdentity,
    xattrs: Vec<String>,
    xattr_values: Vec<String>,
    provider_identifier: Option<String>,
    source: String,
}

impl CloudHints {
    fn read(path: &Path) -> Self {
        Self::read_with_optional_identity(path, None)
    }

    fn read_checked(path: &Path, check: &mut impl FnMut() -> Result<()>) -> Result<Self> {
        Self::read_with_optional_identity_checked(path, None, check)
    }

    fn read_with_identity(path: &Path) -> Self {
        let hints = Self::read(path);
        if should_query_native_fileprovider_identity(path, &hints) {
            Self::read_with_optional_identity(path, Some(copy_fileprovider_identity(path)))
        } else {
            hints
        }
    }

    fn read_with_identity_checked(
        path: &Path,
        check: &mut impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check()?;
        let hints = Self::read_checked(path, check)?;
        check()?;
        if should_query_native_fileprovider_identity(path, &hints) {
            check()?;
            let identity = copy_fileprovider_identity(path);
            check()?;
            Self::read_with_optional_identity_checked(path, Some(identity), check)
        } else {
            Ok(hints)
        }
    }

    fn read_with_optional_identity(
        path: &Path,
        native_identity: Option<NativeFileProviderIdentity>,
    ) -> Self {
        Self::read_with_optional_identity_checked(path, native_identity, &mut || Ok(()))
            .expect("non-cancellable FileProvider hint read cannot cancel")
    }

    fn read_with_optional_identity_checked(
        path: &Path,
        native_identity: Option<NativeFileProviderIdentity>,
        check: &mut impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check()?;
        let native = copy_fileprovider_resource_values(path);
        check()?;
        let native_identity = native_identity.unwrap_or_else(identity_not_queried);
        let mut xattrs = Vec::new();
        let mut xattr_values = Vec::new();
        let mut provider_identifier = None;
        let mut sources = Vec::new();
        let path_sources = provider_path_sources(path);
        check()?;

        if native_has_fileprovider_values(&native) {
            sources.push("native-url-resource".to_string());
        }
        if native_identity.status == NativeFileProviderIdentityStatus::Available {
            sources.push("nsfileprovidermanager".to_string());
            provider_identifier = native_identity.domain_identifier.clone();
        }

        if should_read_provider_xattrs(&native, &native_identity, !path_sources.is_empty()) {
            check()?;
            if let Ok(attrs) = xattr::list(path) {
                for attr in attrs {
                    check()?;
                    let attr = attr.to_string_lossy().to_string();
                    if attr.contains("icloud")
                        || attr.contains("fileprovider")
                        || attr.contains("ubiquit")
                    {
                        check()?;
                        if let Some(value) = xattr_string_value(path, &attr) {
                            check()?;
                            if provider_identifier.is_none() {
                                provider_identifier =
                                    provider_identifier_from_xattr_value(&attr, &value);
                            }
                            xattr_values.push(value);
                        }
                        xattrs.push(attr);
                    }
                }
                if !xattrs.is_empty() {
                    sources.push("xattr".to_string());
                }
            }
        }

        if should_include_provider_path_sources(&native, &native_identity) {
            sources.extend(path_sources);
        }
        check()?;

        Ok(Self {
            native,
            native_identity,
            xattrs,
            xattr_values,
            provider_identifier,
            source: if sources.is_empty() {
                "filesystem".to_string()
            } else {
                sources.sort();
                sources.dedup();
                sources.join("+")
            },
        })
    }
}

fn provider_path_sources(path: &Path) -> Vec<String> {
    let mut sources = Vec::new();
    if path_components(path)
        .iter()
        .any(|component| component == ICLOUD_DRIVE_COMPONENT)
    {
        sources.push("icloud-path".to_string());
    }
    if path.extension().and_then(|value| value.to_str()) == Some("icloud") {
        sources.push("icloud-extension".to_string());
    }
    let name = file_name_lower(path);
    if name.contains("icloud") || name.contains("fileprovider") {
        sources.push("fixture-name".to_string());
    }
    sources
}

fn should_read_provider_xattrs(
    native: &NativeFileProviderResourceValues,
    native_identity: &NativeFileProviderIdentity,
    has_path_hint: bool,
) -> bool {
    if native_resource_proves_local_only(native, native_identity) {
        return false;
    }
    has_path_hint
        || native_identity.status == NativeFileProviderIdentityStatus::Available
        || native.status != gfm_mac_sys::NativeFileProviderStatus::Available
        || native.is_ubiquitous == Some(true)
        || native_has_ubiquitous_materialization_evidence(native)
}

fn should_include_provider_path_sources(
    native: &NativeFileProviderResourceValues,
    native_identity: &NativeFileProviderIdentity,
) -> bool {
    !native_resource_proves_local_only(native, native_identity)
}

fn should_query_native_fileprovider_identity(path: &Path, hints: &CloudHints) -> bool {
    if native_resource_proves_local_only(&hints.native, &hints.native_identity) {
        return false;
    }
    hints.native.is_ubiquitous == Some(true)
        || native_has_ubiquitous_materialization_evidence(&hints.native)
        || hints.provider_identifier.is_some()
        || hints
            .xattrs
            .iter()
            .any(|attr| attr.contains("fileprovider") || attr.contains("ubiquit"))
        || path_components(path)
            .iter()
            .any(|component| component == ICLOUD_DRIVE_COMPONENT)
        || path.extension().and_then(|value| value.to_str()) == Some("icloud")
}

fn identity_not_queried() -> NativeFileProviderIdentity {
    NativeFileProviderIdentity {
        status: NativeFileProviderIdentityStatus::NotQueried,
        item_identifier: None,
        domain_identifier: None,
        reason: Some("nsfileprovidermanager-identity-not-queried-on-hot-path".to_string()),
    }
}

fn domain_for_path(path: &Path, hints: &CloudHints) -> FileProviderDomain {
    if native_proves_local_only(hints) {
        return FileProviderDomain::Local;
    }
    if hints
        .native_identity
        .domain_identifier
        .as_deref()
        .is_some_and(is_icloud_domain_identifier)
        || hints.native.is_ubiquitous == Some(true)
        || native_has_ubiquitous_materialization_evidence(&hints.native)
        || path_components(path)
            .iter()
            .any(|component| component == ICLOUD_DRIVE_COMPONENT)
        || evidence_backed_icloud_name_hint(path, hints)
        || hints
            .xattrs
            .iter()
            .any(|attr| attr.contains("icloud") || attr.contains("ubiquit"))
    {
        FileProviderDomain::ICloudDrive
    } else if hints.native_identity.status == NativeFileProviderIdentityStatus::Available
        || hints
            .xattrs
            .iter()
            .any(|attr| attr.contains("fileprovider"))
        || (native_provider_state_unavailable(hints)
            && file_name_lower(path).contains("fileprovider"))
    {
        FileProviderDomain::FileProvider
    } else {
        FileProviderDomain::Local
    }
}

fn evidence_backed_icloud_name_hint(path: &Path, hints: &CloudHints) -> bool {
    file_name_lower(path).contains("icloud")
        && (hints
            .xattrs
            .iter()
            .any(|attr| attr.contains("fileprovider"))
            || native_provider_state_unavailable(hints))
}

fn native_proves_local_only(hints: &CloudHints) -> bool {
    native_resource_proves_local_only(&hints.native, &hints.native_identity)
}

fn native_resource_proves_local_only(
    native: &NativeFileProviderResourceValues,
    native_identity: &NativeFileProviderIdentity,
) -> bool {
    native.status == gfm_mac_sys::NativeFileProviderStatus::Available
        && (native.is_ubiquitous == Some(false) || native.is_excluded_from_sync == Some(true))
        && !native_has_ubiquitous_materialization_evidence(native)
        && native_identity.status != NativeFileProviderIdentityStatus::Available
}

fn native_provider_state_unavailable(hints: &CloudHints) -> bool {
    matches!(
        hints.native.status,
        gfm_mac_sys::NativeFileProviderStatus::UnsupportedPath
            | gfm_mac_sys::NativeFileProviderStatus::Missing
            | gfm_mac_sys::NativeFileProviderStatus::Unavailable
    ) || matches!(
        hints.native_identity.status,
        NativeFileProviderIdentityStatus::ProviderUnavailable
            | NativeFileProviderIdentityStatus::TimedOut
            | NativeFileProviderIdentityStatus::Failed
            | NativeFileProviderIdentityStatus::Unavailable
            | NativeFileProviderIdentityStatus::Missing
            | NativeFileProviderIdentityStatus::UnsupportedPath
    )
}

fn is_icloud_domain_identifier(identifier: &str) -> bool {
    let identifier = identifier.to_ascii_lowercase();
    identifier.contains("icloud")
        || identifier.contains("clouddocs")
        || identifier.contains("cloudkit")
        || identifier.contains("ubiquit")
}

fn storage_state_for_path(
    path: &Path,
    domain: FileProviderDomain,
    hints: &CloudHints,
) -> CloudStorageState {
    if domain == FileProviderDomain::Local {
        return CloudStorageState::LocalOnly;
    }

    if hints.native.is_ubiquitous == Some(true)
        || native_has_ubiquitous_materialization_evidence(&hints.native)
    {
        if native_has_offline_error(&hints.native) {
            return CloudStorageState::Offline;
        }
        if let Some(state) = native_storage_state(&hints.native) {
            return state;
        }
        if hints.native.is_ubiquitous == Some(true) {
            return CloudStorageState::Unknown;
        }
    }

    if let Some(state) = xattr_storage_state(hints) {
        return state;
    }

    let name = file_name_lower(path);
    let allow_name_state_hints = provider_state_name_hints_allowed(hints);
    let attr_blob = xattr_signal_blob(hints);
    if (allow_name_state_hints && name.contains("conflict"))
        || contains_state_phrase_without_false_marker(
            &attr_blob,
            &["unresolved-conflict", "unresolved conflict", "conflict"],
            &[
                "conflict",
                "conflicts",
                "unresolved-conflicts",
                "hasunresolvedconflicts",
            ],
        )
    {
        CloudStorageState::Conflict
    } else if (allow_name_state_hints && name.contains("offline"))
        || contains_state_phrase_without_false_marker(
            &attr_blob,
            &["offline", "network-unavailable", "network unavailable"],
            &["offline", "network-offline"],
        )
    {
        CloudStorageState::Offline
    } else if has_evicted_materialization_evidence(path, hints)
        || attr_blob.contains("placeholder")
        || attr_blob.contains("evict")
    {
        CloudStorageState::Evicted
    } else if (allow_name_state_hints && name.contains("downloading"))
        || contains_state_phrase_without_false_marker(
            &attr_blob,
            &[
                "download-in-progress",
                "download in progress",
                "downloading",
            ],
            &["downloading", "isdownloading"],
        )
    {
        CloudStorageState::Downloading
    } else if (allow_name_state_hints && name.contains("uploading"))
        || contains_state_phrase_without_false_marker(
            &attr_blob,
            &["upload-in-progress", "upload in progress", "uploading"],
            &["uploading", "isuploading"],
        )
    {
        CloudStorageState::Uploading
    } else if (allow_name_state_hints && name.contains("waiting"))
        || contains_state_phrase_without_false_marker(
            &attr_blob,
            &["waiting", "queued", "requested"],
            &["waiting", "queued", "requested", "downloadrequested"],
        )
    {
        CloudStorageState::Waiting
    } else if (hints.native_identity.status == NativeFileProviderIdentityStatus::Available
        && !native_has_ubiquitous_materialization_evidence(&hints.native))
        || (domain == FileProviderDomain::FileProvider
            && !native_has_fileprovider_values(&hints.native))
        || (path_only_provider_hint(&hints.source) && hints.xattrs.is_empty())
    {
        CloudStorageState::Unknown
    } else if path.try_exists().ok() == Some(true) {
        CloudStorageState::Downloaded
    } else {
        CloudStorageState::Unknown
    }
}

fn provider_state_name_hints_allowed(hints: &CloudHints) -> bool {
    hints.native_identity.status == NativeFileProviderIdentityStatus::Available
        || native_has_ubiquitous_materialization_evidence(&hints.native)
        || !hints.xattrs.is_empty()
        || !hints.xattr_values.is_empty()
        || !native_provider_state_unavailable(hints)
}

fn xattr_storage_state(hints: &CloudHints) -> Option<CloudStorageState> {
    let blob = hints
        .xattr_values
        .iter()
        .map(|value| value.as_str())
        .collect::<Vec<_>>()
        .join("\n")
        .to_ascii_lowercase();
    if blob.is_empty() {
        return None;
    }
    if has_truthy_marker(
        &blob,
        &[
            "conflict",
            "conflicts",
            "unresolved-conflicts",
            "hasunresolvedconflicts",
        ],
    ) || contains_state_phrase_without_false_marker(
        &blob,
        &["unresolved-conflict", "unresolved conflict", "conflict"],
        &[
            "conflict",
            "conflicts",
            "unresolved-conflicts",
            "hasunresolvedconflicts",
        ],
    ) {
        Some(CloudStorageState::Conflict)
    } else if has_truthy_marker(&blob, &["offline", "network-offline"])
        || contains_state_phrase_without_false_marker(
            &blob,
            &["offline", "network-unavailable", "network unavailable"],
            &["offline", "network-offline"],
        )
    {
        Some(CloudStorageState::Offline)
    } else if has_truthy_marker(&blob, &["downloading", "isdownloading"])
        || contains_state_phrase_without_false_marker(
            &blob,
            &[
                "download-in-progress",
                "download in progress",
                "downloading",
            ],
            &["downloading", "isdownloading"],
        )
    {
        Some(CloudStorageState::Downloading)
    } else if has_truthy_marker(&blob, &["uploading", "isuploading"])
        || contains_state_phrase_without_false_marker(
            &blob,
            &["upload-in-progress", "upload in progress", "uploading"],
            &["uploading", "isuploading"],
        )
    {
        Some(CloudStorageState::Uploading)
    } else if has_truthy_marker(
        &blob,
        &["waiting", "queued", "requested", "downloadrequested"],
    ) || contains_state_phrase_without_false_marker(
        &blob,
        &["waiting", "queued", "requested"],
        &["waiting", "queued", "requested", "downloadrequested"],
    ) {
        Some(CloudStorageState::Waiting)
    } else if blob.contains("not-downloaded")
        || blob.contains("not_downloaded")
        || blob.contains("not downloaded")
        || blob.contains("evicted")
        || blob.contains("placeholder")
        || has_false_marker(
            &blob,
            &["downloaded", "isdownloaded", "current", "materialized"],
        )
    {
        Some(CloudStorageState::Evicted)
    } else if has_truthy_marker(
        &blob,
        &["downloaded", "isdownloaded", "current", "materialized"],
    ) || contains_state_phrase_without_false_marker(
        &blob,
        &["downloaded", "current", "materialized"],
        &["downloaded", "isdownloaded", "current", "materialized"],
    ) {
        Some(CloudStorageState::Downloaded)
    } else if has_truthy_marker(&blob, &["unknown", "unknown-provider-state"])
        || contains_state_phrase_without_false_marker(
            &blob,
            &["unknown-provider-state", "unknown provider state"],
            &["unknown", "unknown-provider-state"],
        )
    {
        Some(CloudStorageState::Unknown)
    } else {
        None
    }
}

fn contains_state_phrase_without_false_marker(
    blob: &str,
    phrases: &[&str],
    false_markers: &[&str],
) -> bool {
    phrases.iter().any(|phrase| blob.contains(phrase)) && !has_false_marker(blob, false_markers)
}

fn has_truthy_marker(blob: &str, names: &[&str]) -> bool {
    names
        .iter()
        .any(|name| has_marker_value(blob, name, &["true", "1", "yes", "on"]))
}

fn has_false_marker(blob: &str, names: &[&str]) -> bool {
    names
        .iter()
        .any(|name| has_marker_value(blob, name, &["false", "0", "no", "off"]))
}

fn has_marker_value(blob: &str, name: &str, values: &[&str]) -> bool {
    let mut offset = 0;
    while let Some(found) = blob[offset..].find(name) {
        let after_name = &blob[offset + found + name.len()..];
        let Some(after_separator) = after_name
            .strip_prefix('=')
            .or_else(|| after_name.strip_prefix(':'))
            .or_else(|| after_name.strip_prefix(' '))
            .or_else(|| after_name.strip_prefix('-'))
            .or_else(|| after_name.strip_prefix('_'))
        else {
            offset += found + name.len();
            continue;
        };
        if values.iter().any(|value| {
            after_separator.starts_with(value)
                && marker_value_has_boundary(&after_separator[value.len()..])
        }) {
            return true;
        }
        offset += found + name.len();
    }
    false
}

fn marker_value_has_boundary(rest: &str) -> bool {
    rest.chars()
        .next()
        .is_none_or(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')))
}

fn path_only_provider_hint(source: &str) -> bool {
    let mut saw_path_hint = false;
    for source in source.split('+') {
        match source {
            "fixture-name" | "icloud-extension" | "icloud-path" => saw_path_hint = true,
            "filesystem" => {}
            _ => return false,
        }
    }
    saw_path_hint
}

fn native_storage_state(values: &NativeFileProviderResourceValues) -> Option<CloudStorageState> {
    if values.is_excluded_from_sync == Some(true) {
        Some(CloudStorageState::LocalOnly)
    } else if values.has_unresolved_conflicts == Some(true) {
        Some(CloudStorageState::Conflict)
    } else if values.is_downloading == Some(true) {
        Some(CloudStorageState::Downloading)
    } else if values.is_uploading == Some(true)
        || (values.is_uploaded != Some(true)
            && values
                .percent_uploaded_milli
                .is_some_and(|percent| percent < 100_000))
    {
        Some(CloudStorageState::Uploading)
    } else if values.is_downloaded == Some(true) {
        Some(CloudStorageState::Downloaded)
    } else if values.is_downloaded == Some(false) {
        Some(CloudStorageState::Evicted)
    } else {
        match values.downloading_status {
            Some(NativeUbiquitousDownloadingStatus::NotDownloaded) => {
                Some(CloudStorageState::Evicted)
            }
            Some(NativeUbiquitousDownloadingStatus::Downloaded)
            | Some(NativeUbiquitousDownloadingStatus::Current) => {
                Some(CloudStorageState::Downloaded)
            }
            Some(NativeUbiquitousDownloadingStatus::Other) => Some(CloudStorageState::Unknown),
            None if values.percent_downloaded_milli == Some(100_000) => {
                Some(CloudStorageState::Downloaded)
            }
            None if values.percent_downloaded_milli == Some(0) => Some(CloudStorageState::Evicted),
            None if values.percent_downloaded_milli.is_some() => {
                Some(CloudStorageState::Downloading)
            }
            None if values.percent_uploaded_milli == Some(100_000) => {
                Some(CloudStorageState::Downloaded)
            }
            None if values.download_requested == Some(true) => Some(CloudStorageState::Waiting),
            None if values.is_uploaded == Some(false) => Some(CloudStorageState::Waiting),
            None if values.is_uploaded == Some(true) => Some(CloudStorageState::Downloaded),
            None => None,
        }
    }
}

fn materialization_for_state(state: CloudStorageState) -> CloudMaterialization {
    match state {
        CloudStorageState::LocalOnly => CloudMaterialization::NotProviderBacked,
        CloudStorageState::Downloaded => CloudMaterialization::Materialized,
        CloudStorageState::Evicted => CloudMaterialization::RemotePlaceholder,
        CloudStorageState::Downloading
        | CloudStorageState::Uploading
        | CloudStorageState::Waiting => CloudMaterialization::InFlight,
        CloudStorageState::Conflict => CloudMaterialization::Conflict,
        CloudStorageState::Offline => CloudMaterialization::Offline,
        CloudStorageState::Unknown | CloudStorageState::Removed => CloudMaterialization::Unknown,
    }
}

fn materialization_source_for_state(
    state: CloudStorageState,
    hints: &CloudHints,
) -> CloudMaterializationSource {
    if state == CloudStorageState::LocalOnly
        && hints.native_identity.status == NativeFileProviderIdentityStatus::NoProviderForPath
    {
        return CloudMaterializationSource::NativeFileProviderIdentityNoProviderForPath;
    }
    if state == CloudStorageState::LocalOnly && !native_proves_local_only(hints) {
        return CloudMaterializationSource::Filesystem;
    }
    if (state == CloudStorageState::LocalOnly && native_proves_local_only(hints))
        || hints.native.is_ubiquitous == Some(true)
        || native_has_ubiquitous_materialization_evidence(&hints.native)
    {
        CloudMaterializationSource::NativeUrlResource
    } else if state == CloudStorageState::Unknown
        && hints.native.status == gfm_mac_sys::NativeFileProviderStatus::UnsupportedPath
    {
        CloudMaterializationSource::NativeUrlResourceUnsupported
    } else if state == CloudStorageState::Unknown
        && hints.native.status == gfm_mac_sys::NativeFileProviderStatus::Missing
    {
        CloudMaterializationSource::NativeUrlResourceMissing
    } else if state == CloudStorageState::Unknown
        && hints.native.status == gfm_mac_sys::NativeFileProviderStatus::Unavailable
    {
        CloudMaterializationSource::NativeUrlResourceUnavailable
    } else if hints.native_identity.status == NativeFileProviderIdentityStatus::Available
        && state == CloudStorageState::Unknown
    {
        CloudMaterializationSource::NativeFileProviderIdentityUnknown
    } else if state == CloudStorageState::Unknown
        && hints.native_identity.status == NativeFileProviderIdentityStatus::Missing
    {
        CloudMaterializationSource::NativeFileProviderIdentityMissing
    } else if state == CloudStorageState::Unknown
        && hints.native_identity.status == NativeFileProviderIdentityStatus::ProviderUnavailable
    {
        CloudMaterializationSource::NativeFileProviderIdentityProviderUnavailable
    } else if state == CloudStorageState::Unknown
        && hints.native_identity.status == NativeFileProviderIdentityStatus::TimedOut
    {
        CloudMaterializationSource::NativeFileProviderIdentityTimedOut
    } else if state == CloudStorageState::Unknown
        && hints.native_identity.status == NativeFileProviderIdentityStatus::Failed
    {
        CloudMaterializationSource::NativeFileProviderIdentityFailed
    } else if state == CloudStorageState::Unknown
        && matches!(
            hints.native_identity.status,
            NativeFileProviderIdentityStatus::ProviderUnavailable
                | NativeFileProviderIdentityStatus::TimedOut
                | NativeFileProviderIdentityStatus::Unavailable
                | NativeFileProviderIdentityStatus::Failed
        )
    {
        CloudMaterializationSource::NativeFileProviderIdentityUnavailable
    } else if state == CloudStorageState::Unknown
        && hints.native_identity.status == NativeFileProviderIdentityStatus::UnsupportedPath
    {
        CloudMaterializationSource::NativeFileProviderIdentityUnsupported
    } else if !hints.xattrs.is_empty() {
        CloudMaterializationSource::XattrFallback
    } else if path_only_provider_hint(&hints.source) {
        CloudMaterializationSource::PathFallback
    } else if hints.source == "filesystem" {
        CloudMaterializationSource::Filesystem
    } else {
        CloudMaterializationSource::StateFallback
    }
}

fn materialization_confidence_for_source(
    source: CloudMaterializationSource,
) -> CloudMaterializationConfidence {
    match source {
        CloudMaterializationSource::NativeUrlResource
        | CloudMaterializationSource::NativeUrlResourceMissing
        | CloudMaterializationSource::NativeUrlResourceUnavailable
        | CloudMaterializationSource::NativeUrlResourceUnsupported => {
            CloudMaterializationConfidence::Native
        }
        CloudMaterializationSource::NativeFileProviderIdentityUnknown
        | CloudMaterializationSource::NativeFileProviderIdentityMissing
        | CloudMaterializationSource::NativeFileProviderIdentityProviderUnavailable
        | CloudMaterializationSource::NativeFileProviderIdentityTimedOut
        | CloudMaterializationSource::NativeFileProviderIdentityUnavailable
        | CloudMaterializationSource::NativeFileProviderIdentityFailed
        | CloudMaterializationSource::NativeFileProviderIdentityUnsupported
        | CloudMaterializationSource::NativeFileProviderIdentityNoProviderForPath => {
            CloudMaterializationConfidence::ProviderIdentity
        }
        CloudMaterializationSource::XattrFallback => CloudMaterializationConfidence::XattrFallback,
        CloudMaterializationSource::PathFallback => CloudMaterializationConfidence::PathFallback,
        CloudMaterializationSource::Filesystem => CloudMaterializationConfidence::Filesystem,
        CloudMaterializationSource::StateFallback => CloudMaterializationConfidence::StateFallback,
    }
}

fn materialization_reason_for_state(
    state: CloudStorageState,
    hints: &CloudHints,
) -> Option<String> {
    if state == CloudStorageState::LocalOnly
        && hints.native_identity.status == NativeFileProviderIdentityStatus::NoProviderForPath
    {
        return hints.native_identity.reason.clone();
    }
    if state == CloudStorageState::LocalOnly && native_proves_local_only(hints) {
        return Some("native-url-resource-not-provider-backed".to_string());
    }
    if hints.native.is_ubiquitous == Some(true)
        || native_has_ubiquitous_materialization_evidence(&hints.native)
    {
        return Some(match state {
            CloudStorageState::Downloaded => "native-url-resource-materialized".to_string(),
            CloudStorageState::Evicted => "native-url-resource-remote-placeholder".to_string(),
            CloudStorageState::Downloading => "native-url-resource-downloading".to_string(),
            CloudStorageState::Uploading => "native-url-resource-uploading".to_string(),
            CloudStorageState::Waiting => "native-url-resource-waiting".to_string(),
            CloudStorageState::Conflict => "native-url-resource-conflict".to_string(),
            CloudStorageState::Offline => hints
                .native
                .downloading_error
                .as_ref()
                .or(hints.native.uploading_error.as_ref())
                .and_then(|error| error.description.clone())
                .unwrap_or_else(|| "native-url-resource-offline".to_string()),
            CloudStorageState::LocalOnly | CloudStorageState::Unknown => {
                "native-url-resource-unknown".to_string()
            }
            CloudStorageState::Removed => "fileprovider-item-removed".to_string(),
        });
    }
    if state == CloudStorageState::Unknown {
        if matches!(
            hints.native.status,
            gfm_mac_sys::NativeFileProviderStatus::UnsupportedPath
                | gfm_mac_sys::NativeFileProviderStatus::Missing
                | gfm_mac_sys::NativeFileProviderStatus::Unavailable
        ) {
            return hints.native.reason.clone();
        }
        if matches!(
            hints.native_identity.status,
            NativeFileProviderIdentityStatus::Available
                | NativeFileProviderIdentityStatus::ProviderUnavailable
                | NativeFileProviderIdentityStatus::TimedOut
                | NativeFileProviderIdentityStatus::Unavailable
                | NativeFileProviderIdentityStatus::Failed
                | NativeFileProviderIdentityStatus::Missing
                | NativeFileProviderIdentityStatus::UnsupportedPath
        ) {
            return hints.native_identity.reason.clone();
        }
    }
    match state {
        CloudStorageState::LocalOnly => Some("not-fileprovider-backed".to_string()),
        CloudStorageState::Downloaded => Some("materialized".to_string()),
        CloudStorageState::Evicted => Some("remote-placeholder".to_string()),
        CloudStorageState::Downloading => Some("provider-download-in-flight".to_string()),
        CloudStorageState::Uploading => Some("provider-upload-in-flight".to_string()),
        CloudStorageState::Waiting => Some("provider-waiting".to_string()),
        CloudStorageState::Conflict => Some("conflict-requires-resolution".to_string()),
        CloudStorageState::Offline => Some("provider-offline".to_string()),
        CloudStorageState::Unknown => Some("unknown-provider-state".to_string()),
        CloudStorageState::Removed => Some("fileprovider-item-removed".to_string()),
    }
}

fn progress_for_state(state: CloudStorageState, hints: &CloudHints) -> CloudTransferProgress {
    match state {
        CloudStorageState::LocalOnly => CloudTransferProgress::idle("not-fileprovider-backed"),
        CloudStorageState::Downloaded => {
            CloudTransferProgress::complete(CloudTransferDirection::Download, "materialized")
        }
        CloudStorageState::Evicted => CloudTransferProgress {
            direction: CloudTransferDirection::Download,
            percent_milli: hints.native.percent_downloaded_milli.or(Some(0)),
            requested: hints.native.download_requested.unwrap_or(false),
            complete: false,
            indeterminate: false,
            source: if hints.native.percent_downloaded_milli.is_some() {
                "native-url-resource"
            } else {
                "state"
            },
            reason: Some("remote-placeholder".to_string()),
        },
        CloudStorageState::Downloading => CloudTransferProgress::from_native(
            CloudTransferDirection::Download,
            hints.native.percent_downloaded_milli,
            hints.native.download_requested.unwrap_or(true),
        ),
        CloudStorageState::Uploading => CloudTransferProgress::from_native(
            CloudTransferDirection::Upload,
            hints.native.percent_uploaded_milli,
            false,
        ),
        CloudStorageState::Waiting => CloudTransferProgress::from_native(
            CloudTransferDirection::Materialize,
            hints.native.percent_downloaded_milli,
            hints.native.download_requested.unwrap_or(false),
        ),
        CloudStorageState::Conflict => CloudTransferProgress::idle("conflict-requires-resolution"),
        CloudStorageState::Offline => CloudTransferProgress::idle("provider-offline"),
        CloudStorageState::Unknown => CloudTransferProgress::idle("unknown-provider-state"),
        CloudStorageState::Removed => CloudTransferProgress::idle("fileprovider-item-removed"),
    }
}

fn badges_for_state(state: CloudStorageState) -> Vec<CloudBadge> {
    match state {
        CloudStorageState::LocalOnly => Vec::new(),
        CloudStorageState::Downloaded => vec![CloudBadge::AvailableOffline],
        CloudStorageState::Evicted => vec![CloudBadge::Cloud],
        CloudStorageState::Downloading => vec![CloudBadge::Cloud, CloudBadge::Downloading],
        CloudStorageState::Uploading => vec![CloudBadge::Uploading],
        CloudStorageState::Waiting => vec![CloudBadge::Waiting],
        CloudStorageState::Conflict => vec![CloudBadge::Conflict],
        CloudStorageState::Offline => vec![CloudBadge::Offline],
        CloudStorageState::Unknown => vec![CloudBadge::Waiting],
        CloudStorageState::Removed => Vec::new(),
    }
}

fn command_policy(
    domain: FileProviderDomain,
    state: CloudStorageState,
    provider_commands_available: bool,
) -> CloudCommandPolicy {
    if domain == FileProviderDomain::Local {
        return CloudCommandPolicy::local();
    }
    if !provider_commands_available
        && matches!(
            state,
            CloudStorageState::Evicted | CloudStorageState::Offline | CloudStorageState::Downloaded
        )
    {
        return CloudCommandPolicy {
            download: CloudCommandState::Disabled,
            evict: CloudCommandState::Disabled,
            reveal_conflict: CloudCommandState::Hidden,
            reason: Some("not-native-provider-backed".to_string()),
        };
    }
    match state {
        CloudStorageState::Evicted | CloudStorageState::Offline => CloudCommandPolicy {
            download: CloudCommandState::Enabled,
            evict: CloudCommandState::Disabled,
            reveal_conflict: CloudCommandState::Hidden,
            reason: None,
        },
        CloudStorageState::Downloaded => CloudCommandPolicy {
            download: CloudCommandState::Disabled,
            evict: CloudCommandState::Enabled,
            reveal_conflict: CloudCommandState::Hidden,
            reason: None,
        },
        CloudStorageState::Conflict => CloudCommandPolicy {
            download: CloudCommandState::Disabled,
            evict: CloudCommandState::Disabled,
            reveal_conflict: CloudCommandState::Enabled,
            reason: Some("conflict-requires-user-resolution".to_string()),
        },
        CloudStorageState::Downloading
        | CloudStorageState::Uploading
        | CloudStorageState::Waiting => CloudCommandPolicy {
            download: CloudCommandState::Disabled,
            evict: CloudCommandState::Disabled,
            reveal_conflict: CloudCommandState::Hidden,
            reason: Some("provider-operation-in-flight".to_string()),
        },
        CloudStorageState::Unknown => CloudCommandPolicy {
            download: CloudCommandState::Disabled,
            evict: CloudCommandState::Disabled,
            reveal_conflict: CloudCommandState::Hidden,
            reason: Some("unknown-provider-state".to_string()),
        },
        CloudStorageState::LocalOnly | CloudStorageState::Removed => CloudCommandPolicy::local(),
    }
}

fn removed_provider_domain_for_path(path: &Path) -> FileProviderDomain {
    if path_components(path)
        .iter()
        .any(|component| component == ICLOUD_DRIVE_COMPONENT)
        || file_name_lower(path).contains("icloud")
        || is_evicted_placeholder_path(path)
    {
        FileProviderDomain::ICloudDrive
    } else {
        FileProviderDomain::FileProvider
    }
}

fn provider_commands_available(hints: &CloudHints) -> bool {
    hints.native.is_ubiquitous == Some(true)
        || native_has_ubiquitous_materialization_evidence(&hints.native)
}

fn is_evicted_placeholder_path(path: &Path) -> bool {
    let name = file_name_lower(path);
    name.ends_with(".icloud")
}

fn has_evicted_materialization_evidence(_path: &Path, hints: &CloudHints) -> bool {
    hints.xattrs.iter().any(|attr| {
        let attr = attr.to_ascii_lowercase();
        attr.contains("placeholder") || attr.contains("evict")
    }) || hints.xattr_values.iter().any(|value| {
        let value = value.to_ascii_lowercase();
        value.contains("not-downloaded")
            || value.contains("not_downloaded")
            || value.contains("placeholder")
            || value.contains("evict")
    })
}

fn xattr_signal_blob(hints: &CloudHints) -> String {
    hints
        .xattrs
        .iter()
        .chain(hints.xattr_values.iter())
        .map(|value| value.as_str())
        .collect::<Vec<_>>()
        .join("\n")
        .to_ascii_lowercase()
}

fn xattr_string_value(path: &Path, attr: &str) -> Option<String> {
    let value = xattr::get(path, attr).ok().flatten()?;
    if value.is_empty() || value.len() > MAX_PROVIDER_XATTR_VALUE_BYTES {
        return None;
    }
    String::from_utf8(value).ok()
}

fn provider_identifier_from_xattr_value(attr: &str, value: &str) -> Option<String> {
    let value = non_empty(value.to_string())?;
    let attr = attr.to_ascii_lowercase();
    let value_lower = value.to_ascii_lowercase();
    if !(attr.contains("domain")
        || attr.contains("provider")
        || attr.contains("identifier")
        || attr.contains("account"))
    {
        return None;
    }
    if value_lower.contains("download")
        || value_lower.contains("placeholder")
        || value_lower.contains("material")
        || value_lower.contains("evict")
        || value_lower.contains("offline")
        || value_lower.contains("conflict")
        || !value.contains('.')
    {
        return None;
    }
    Some(value)
}

fn matched_domain<'a>(
    hints: &CloudHints,
    domains: &'a NativeFileProviderDomainEnumeration,
) -> Option<&'a NativeFileProviderDomain> {
    let identifier = hints.native_identity.domain_identifier.as_deref()?;
    domains
        .domains
        .iter()
        .find(|domain| domain.identifier.as_deref() == Some(identifier))
}

fn path_components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .map(ToOwned::to_owned)
        .collect()
}

fn file_name_lower(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn non_empty(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn native_has_fileprovider_values(values: &NativeFileProviderResourceValues) -> bool {
    values.is_ubiquitous.is_some()
        || values.has_unresolved_conflicts.is_some()
        || values.is_downloaded.is_some()
        || values.is_downloading.is_some()
        || values.is_uploading.is_some()
        || values.is_uploaded.is_some()
        || values.download_requested.is_some()
        || values.percent_downloaded_milli.is_some()
        || values.percent_uploaded_milli.is_some()
        || values.downloading_status.is_some()
        || values.downloading_error.is_some()
        || values.uploading_error.is_some()
        || values.is_excluded_from_sync.is_some()
}

fn native_has_offline_error(values: &NativeFileProviderResourceValues) -> bool {
    values
        .downloading_error
        .as_ref()
        .or(values.uploading_error.as_ref())
        .is_some_and(|error| {
            error.code == Some(4_355)
                || error.description.as_deref().is_some_and(|description| {
                    let description = description.to_ascii_lowercase();
                    description.contains("server")
                        && (description.contains("unavailable") || description.contains("failed"))
                        || description.contains("offline")
                        || description.contains("network")
                })
        })
}

fn native_has_ubiquitous_materialization_evidence(
    values: &NativeFileProviderResourceValues,
) -> bool {
    if values.is_ubiquitous == Some(false) || values.is_excluded_from_sync == Some(true) {
        return false;
    }
    native_storage_state(values).is_some() || native_has_offline_error(values)
}

fn escape_field(value: &str) -> String {
    let mut output = String::new();
    for ch in value.chars() {
        match ch {
            '\\' => output.push_str("\\\\"),
            '\t' => output.push_str("\\t"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            other => output.push(other),
        }
    }
    output
}

fn unescape_field(value: &str) -> String {
    let mut output = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        match chars.next() {
            Some('\\') => output.push('\\'),
            Some('t') => output.push('\t'),
            Some('n') => output.push('\n'),
            Some('r') => output.push('\r'),
            Some(other) => {
                output.push('\\');
                output.push(other);
            }
            None => output.push('\\'),
        }
    }
    output
}

fn atomic_write_text_checked(
    path: &Path,
    text: &str,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    check_control()?;
    let temporary = temporary_path(path);
    let mut file = File::create(&temporary).map_err(|err| GfmError::io(&temporary, err))?;
    if let Err(err) = file.write_all(text.as_bytes()) {
        let _ = fs::remove_file(&temporary);
        return Err(GfmError::io(&temporary, err));
    }
    if let Err(err) = check_control() {
        let _ = fs::remove_file(&temporary);
        return Err(err);
    }
    if let Err(err) = file.sync_all() {
        let _ = fs::remove_file(&temporary);
        return Err(GfmError::io(&temporary, err));
    }
    if let Err(err) = check_control() {
        let _ = fs::remove_file(&temporary);
        return Err(err);
    }
    drop(file);
    if let Err(err) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(GfmError::io(path, err));
    }
    let _ = sync_parent(path);
    Ok(())
}

fn sync_parent(path: &Path) -> Result<bool> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    match File::open(parent) {
        Ok(file) => Ok(file.sync_all().is_ok()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
        Err(err) => Err(GfmError::io(parent, err)),
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("fileprovider-state");
    path.with_file_name(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        temp_nonce()
    ))
}

fn temp_nonce() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NONCE: AtomicU64 = AtomicU64::new(0);
    NONCE.fetch_add(1, Ordering::Relaxed)
}

fn affected_paths_field(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        "-".to_string()
    } else {
        paths
            .iter()
            .map(|path| escape_field(&path.display().to_string()))
            .collect::<Vec<_>>()
            .join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_mac_sys::{NativeFileProviderStatus, NativeUbiquitousError};
    #[cfg(unix)]
    use std::ffi::OsString;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
    static CWD_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn reports_path_only_icloud_file_as_local_without_native_evidence() {
        let root = unique_temp_dir();
        let path = root.join("Downloaded.icloud.md");
        fs::write(&path, "downloaded").unwrap();

        let report = FileProviderStateReport::read_path(&path).unwrap();

        assert_eq!(report.domain, FileProviderDomain::Local);
        assert_eq!(report.storage_state, CloudStorageState::LocalOnly);
        assert_eq!(
            report.materialization,
            CloudMaterialization::NotProviderBacked
        );
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::Filesystem
        );
        assert_eq!(
            report.materialization_confidence,
            CloudMaterializationConfidence::Filesystem
        );
        assert_eq!(report.progress.direction, CloudTransferDirection::Idle);
        assert_eq!(report.progress.percent_milli, None);
        assert!(!report.progress.complete);
        assert_eq!(
            report.progress.reason.as_deref(),
            Some("not-fileprovider-backed")
        );
        assert!(report.badges.is_empty());
        assert_eq!(report.commands.evict, CloudCommandState::Hidden);
        assert_eq!(report.commands.download, CloudCommandState::Hidden);
        assert_eq!(
            report.commands.reason.as_deref(),
            Some("not-fileprovider-backed")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_path_only_placeholder_as_local_without_provider_evidence() {
        let root = unique_temp_dir();
        let path = root.join("Evicted.icloud-placeholder");
        fs::write(&path, "placeholder").unwrap();

        let report = FileProviderStateReport::read_path(&path).unwrap();

        assert_eq!(report.domain, FileProviderDomain::Local);
        assert_eq!(report.storage_state, CloudStorageState::LocalOnly);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::Filesystem
        );
        assert_eq!(
            report.materialization_confidence,
            CloudMaterializationConfidence::Filesystem
        );
        assert_eq!(report.progress.direction, CloudTransferDirection::Idle);
        assert_eq!(report.progress.percent_milli, None);
        assert_eq!(
            report.progress.reason.as_deref(),
            Some("not-fileprovider-backed")
        );
        assert!(report.badges.is_empty());
        assert!(!report.offline);
        assert_eq!(report.commands.download, CloudCommandState::Hidden);
        assert_eq!(report.commands.evict, CloudCommandState::Hidden);
        assert_eq!(
            report.commands.reason.as_deref(),
            Some("not-fileprovider-backed")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_icloud_path_only_hint_as_path_fallback_without_native_evidence() {
        let path =
            PathBuf::from("/Users/test/Library/Mobile Documents/com~apple~CloudDocs/Report.md");
        let hints = CloudHints {
            native: native_values(),
            native_identity: identity_not_queried(),
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "icloud-path".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.domain, FileProviderDomain::ICloudDrive);
        assert_eq!(report.storage_state, CloudStorageState::Unknown);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::PathFallback
        );
        assert_eq!(
            report.materialization_confidence,
            CloudMaterializationConfidence::PathFallback
        );
        assert_eq!(
            report.materialization_reason.as_deref(),
            Some("unknown-provider-state")
        );
        assert_eq!(report.commands.download, CloudCommandState::Disabled);
        assert_eq!(report.commands.evict, CloudCommandState::Disabled);
    }

    #[test]
    fn icloud_extension_without_materialization_evidence_stays_unknown_when_native_missing() {
        let path = PathBuf::from("/tmp/Remote.icloud");
        let mut native = native_values();
        native.status = NativeFileProviderStatus::Missing;
        native.reason = Some("native FileProvider URL resource path missing".to_string());
        let hints = CloudHints {
            native,
            native_identity: identity_not_queried(),
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "icloud-extension".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.domain, FileProviderDomain::ICloudDrive);
        assert_eq!(report.storage_state, CloudStorageState::Unknown);
        assert_eq!(report.materialization, CloudMaterialization::Unknown);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::NativeUrlResourceMissing
        );
        assert_eq!(
            report.materialization_confidence,
            CloudMaterializationConfidence::Native
        );
        assert_eq!(
            report.materialization_reason.as_deref(),
            Some("native FileProvider URL resource path missing")
        );
        assert_eq!(report.commands.download, CloudCommandState::Disabled);
        assert_eq!(report.commands.evict, CloudCommandState::Disabled);
    }

    #[test]
    fn reports_xattr_evicted_placeholder_without_provider_command() {
        let root = unique_temp_dir();
        let path = root.join("Evicted.icloud-placeholder");
        fs::write(&path, "placeholder").unwrap();
        xattr::set(&path, "com.apple.icloud.placeholder", b"1").unwrap();

        let report = FileProviderStateReport::read_path(&path).unwrap();

        assert_eq!(report.storage_state, CloudStorageState::Evicted);
        assert_eq!(
            report.materialization_confidence,
            CloudMaterializationConfidence::XattrFallback
        );
        assert_eq!(report.progress.direction, CloudTransferDirection::Download);
        assert_eq!(report.progress.percent_milli, Some(0));
        assert!(!report.progress.requested);
        assert!(!report.progress.indeterminate);
        assert_eq!(report.badges, vec![CloudBadge::Cloud]);
        assert!(report.offline);
        assert_eq!(report.commands.download, CloudCommandState::Disabled);
        assert_eq!(report.commands.evict, CloudCommandState::Disabled);
        assert_eq!(
            report.commands.reason.as_deref(),
            Some("not-native-provider-backed")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_local_false_overrides_xattr_placeholder_fallback() {
        let path = PathBuf::from("/tmp/Evicted.icloud-placeholder");
        let mut native = native_values();
        native.is_ubiquitous = Some(false);
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: vec!["com.apple.icloud.placeholder".to_string()],
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "fixture-name+native-url-resource+xattr".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.domain, FileProviderDomain::Local);
        assert_eq!(report.storage_state, CloudStorageState::LocalOnly);
        assert_eq!(
            report.materialization,
            CloudMaterialization::NotProviderBacked
        );
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::NativeUrlResource
        );
        assert_eq!(
            report.materialization_confidence,
            CloudMaterializationConfidence::Native
        );
        assert_eq!(
            report.materialization_reason.as_deref(),
            Some("native-url-resource-not-provider-backed")
        );
        assert_eq!(report.commands.download, CloudCommandState::Hidden);
        assert_eq!(report.commands.evict, CloudCommandState::Hidden);
        assert_eq!(
            report.commands.reason.as_deref(),
            Some("not-fileprovider-backed")
        );
    }

    #[test]
    fn native_excluded_from_sync_overrides_placeholder_fallback() {
        let path = PathBuf::from("/tmp/Evicted.icloud-placeholder");
        let mut native = native_values();
        native.is_excluded_from_sync = Some(true);
        native.has_unresolved_conflicts = Some(false);
        native.is_downloaded = Some(false);
        native.percent_downloaded_milli = Some(0);
        native.downloading_status = Some(NativeUbiquitousDownloadingStatus::NotDownloaded);
        let hints = CloudHints {
            native,
            native_identity: identity_not_queried(),
            xattrs: vec!["com.apple.icloud.placeholder".to_string()],
            xattr_values: vec!["not-downloaded".to_string()],
            provider_identifier: None,
            source: "fixture-name+native-url-resource+xattr".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.domain, FileProviderDomain::Local);
        assert_eq!(report.storage_state, CloudStorageState::LocalOnly);
        assert_eq!(
            report.materialization,
            CloudMaterialization::NotProviderBacked
        );
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::NativeUrlResource
        );
        assert_eq!(
            report.materialization_reason.as_deref(),
            Some("native-url-resource-not-provider-backed")
        );
        assert_eq!(report.commands.download, CloudCommandState::Hidden);
        assert_eq!(report.commands.evict, CloudCommandState::Hidden);
    }

    #[test]
    fn native_materialization_evidence_keeps_provider_commands_available() {
        let path = PathBuf::from("/tmp/Remote.pdf");
        let mut native = native_values();
        native.has_unresolved_conflicts = Some(false);
        native.is_downloaded = Some(false);
        native.is_downloading = Some(false);
        native.is_uploading = Some(false);
        native.download_requested = Some(false);
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "native-url-resource".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.domain, FileProviderDomain::ICloudDrive);
        assert_eq!(report.storage_state, CloudStorageState::Evicted);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::NativeUrlResource
        );
        assert_eq!(report.commands.download, CloudCommandState::Enabled);
        assert_eq!(report.commands.evict, CloudCommandState::Disabled);
        assert_eq!(report.commands.reason, None);
    }

    #[test]
    fn native_ubiquitous_false_suppresses_stray_materialization_keys() {
        let path = PathBuf::from("/tmp/Remote.icloud.md");
        let mut native = native_values();
        native.is_ubiquitous = Some(false);
        native.has_unresolved_conflicts = Some(false);
        native.is_downloaded = Some(false);
        native.is_downloading = Some(false);
        native.is_uploading = Some(false);
        native.download_requested = Some(true);
        native.percent_downloaded_milli = Some(0);
        native.downloading_status = Some(NativeUbiquitousDownloadingStatus::NotDownloaded);
        let hints = CloudHints {
            native,
            native_identity: identity_not_queried(),
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "fixture-name+native-url-resource".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.domain, FileProviderDomain::Local);
        assert_eq!(report.storage_state, CloudStorageState::LocalOnly);
        assert_eq!(
            report.materialization,
            CloudMaterialization::NotProviderBacked
        );
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::NativeUrlResource
        );
        assert_eq!(
            report.materialization_reason.as_deref(),
            Some("native-url-resource-not-provider-backed")
        );
        assert_eq!(report.commands.download, CloudCommandState::Hidden);
        assert_eq!(report.commands.evict, CloudCommandState::Hidden);
    }

    #[test]
    fn native_ubiquitous_false_with_provider_identity_stays_provider_unknown() {
        let path = PathBuf::from("/tmp/Remote.fileprovider");
        let mut native = native_values();
        native.is_ubiquitous = Some(false);
        native.is_downloaded = Some(false);
        native.percent_downloaded_milli = Some(0);
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::Available,
                item_identifier: Some("item-1".to_string()),
                domain_identifier: Some("com.example.provider".to_string()),
                reason: None,
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: Some("com.example.provider".to_string()),
            source: "native-url-resource+nsfileprovidermanager".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.domain, FileProviderDomain::FileProvider);
        assert_eq!(report.storage_state, CloudStorageState::Unknown);
        assert_eq!(report.materialization, CloudMaterialization::Unknown);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::NativeFileProviderIdentityUnknown
        );
        assert_eq!(
            report.materialization_confidence,
            CloudMaterializationConfidence::ProviderIdentity
        );
        assert_eq!(report.commands.download, CloudCommandState::Disabled);
        assert_eq!(report.commands.evict, CloudCommandState::Disabled);
    }

    #[test]
    fn reports_conflict_with_resolution_command() {
        let root = unique_temp_dir();
        let path = root.join("Conflict.icloud-conflict.md");
        fs::write(&path, "conflict").unwrap();
        xattr::set(&path, "com.apple.fileprovider.state", b"conflict").unwrap();

        let report = FileProviderStateReport::read_path(&path).unwrap();

        assert_eq!(report.storage_state, CloudStorageState::Conflict);
        assert_eq!(report.badges, vec![CloudBadge::Conflict]);
        assert!(report.conflict);
        assert_eq!(report.commands.reveal_conflict, CloudCommandState::Enabled);
        assert!(report
            .as_tsv()
            .contains("reason=conflict-requires-user-resolution"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_fileprovider_conflict_resolution_intent() {
        let root = unique_temp_dir();
        let path = root.join("Conflict.icloud-conflict.md");
        fs::write(&path, "conflict").unwrap();
        xattr::set(&path, "com.apple.fileprovider.state", b"conflict").unwrap();

        let report = FileProviderConflictReport::read_path(&path).unwrap();

        assert!(report.has_unresolved_conflict);
        assert_eq!(report.state.storage_state, CloudStorageState::Conflict);
        assert_eq!(report.affected_paths, vec![path.clone()]);
        assert_eq!(report.reveal_command, CloudCommandState::Enabled);
        assert!(report.block_operations);
        assert_eq!(
            affected_paths_field(&report.affected_paths),
            escape_field(&path.display().to_string())
        );
        assert!(report
            .as_tsv()
            .contains("\tconflict=true\tstate=conflict\taffected=1\taffected-paths="));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fileprovider_conflict_report_is_explicit_for_non_conflict_paths() {
        let root = unique_temp_dir();
        let path = root.join("Downloaded.icloud.md");
        fs::write(&path, "downloaded").unwrap();

        let report = FileProviderConflictReport::read_path(&path).unwrap();

        assert!(!report.has_unresolved_conflict);
        assert!(report.affected_paths.is_empty());
        assert_eq!(report.reveal_command, CloudCommandState::Hidden);
        assert!(!report.block_operations);
        assert_eq!(report.reason, "not-fileprovider-backed");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_files_hide_cloud_commands() {
        let root = unique_temp_dir();
        let path = root.join("Local.md");
        fs::write(&path, "local").unwrap();

        let report = FileProviderStateReport::read_path(&path).unwrap();

        assert_eq!(report.domain, FileProviderDomain::Local);
        assert_eq!(report.storage_state, CloudStorageState::LocalOnly);
        assert!(report.badges.is_empty());
        assert_eq!(report.commands.download, CloudCommandState::Hidden);
        assert_eq!(report.commands.evict, CloudCommandState::Hidden);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fileprovider_state_snapshot_round_trips_escaped_paths() {
        let root = unique_temp_dir();
        let path = root.join("fileprovider-state.tsv");
        let snapshot = FileProviderStateSnapshot {
            entries: vec![FileProviderStateSnapshotEntry {
                path: root.join("Remote\tName\nArchive\\2026.icloud"),
                state: CloudStorageState::Evicted,
            }],
        };

        snapshot.write(&path).unwrap();
        let encoded = fs::read_to_string(&path).unwrap();
        assert!(encoded.contains("Remote\\tName\\nArchive\\\\2026.icloud"));
        let reloaded = FileProviderStateSnapshot::read(&path).unwrap();

        assert_eq!(reloaded, snapshot);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fileprovider_state_snapshot_reports_invalid_state_with_line_number() {
        let root = unique_temp_dir();
        let path = root.join("fileprovider-state.tsv");
        fs::write(
            &path,
            "gfm-fileprovider-state-v1\nevicted\t/Cloud/Remote.icloud\nbroken\t/Cloud/Broken.icloud\n",
        )
        .unwrap();

        let err = FileProviderStateSnapshot::read(&path).unwrap_err();

        assert!(err.to_string().contains(&format!("{}:3", path.display())));
        assert!(err
            .to_string()
            .contains("invalid FileProvider state `broken`"));
        assert!(err
            .to_string()
            .contains("unsupported FileProvider storage state `broken`"));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn checked_state_snapshot_read_honors_pre_cancelled_control_before_file_open() {
        let path = PathBuf::from(OsString::from_vec(
            b"/tmp/gfm-fileprovider-snapshot-cancelled\0path".to_vec(),
        ));

        let err = FileProviderStateSnapshot::read_checked(&path, || Err(GfmError::Cancelled))
            .unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
    }

    #[test]
    fn checked_state_snapshot_read_can_cancel_during_parse() {
        let root = unique_temp_dir();
        let path = root.join("fileprovider-state.tsv");
        fs::write(
            &path,
            format!(
                "gfm-fileprovider-state-v1\nevicted\t{}\ndownloaded\t{}\n",
                escape_field(&root.join("Remote.icloud-placeholder").to_string_lossy()),
                escape_field(&root.join("Remote.icloud-downloaded").to_string_lossy())
            ),
        )
        .unwrap();
        let mut checks = 0usize;

        let err = FileProviderStateSnapshot::read_checked(&path, || {
            checks += 1;
            if checks >= 4 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        })
        .unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
        assert!(checks >= 4);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fileprovider_state_snapshot_rejects_duplicate_paths_with_line_number() {
        let root = unique_temp_dir();
        let path = root.join("fileprovider-state.tsv");
        let remote = root.join("Remote.icloud-placeholder");
        fs::write(
            &path,
            format!(
                "gfm-fileprovider-state-v1\nevicted\t{}\ndownloaded\t{}\n",
                remote.display(),
                remote.display()
            ),
        )
        .unwrap();

        let err = FileProviderStateSnapshot::read(&path).unwrap_err();

        assert!(err.to_string().contains(&format!("{}:3", path.display())));
        assert!(err
            .to_string()
            .contains("duplicate FileProvider state path"));
        assert!(err.to_string().contains(&remote.display().to_string()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fileprovider_state_snapshot_skips_name_only_local_paths() {
        let root = unique_temp_dir();
        let local = root.join("Downloaded.icloud.md");
        let evicted = root.join("Remote.icloud-placeholder");
        fs::write(&local, "local").unwrap();
        fs::write(&evicted, "placeholder").unwrap();
        mark_evicted_fixture(&evicted);

        let snapshot =
            FileProviderStateSnapshot::from_paths([local.clone(), evicted.clone()]).unwrap();

        assert_eq!(
            snapshot.entries,
            vec![FileProviderStateSnapshotEntry {
                path: evicted,
                state: CloudStorageState::Evicted,
            }]
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fileprovider_state_snapshot_deduplicates_input_paths_before_persisting() {
        let root = unique_temp_dir();
        let evicted = root.join("Remote.icloud-placeholder");
        fs::write(&evicted, "placeholder").unwrap();
        mark_evicted_fixture(&evicted);

        let snapshot = FileProviderStateSnapshot::from_paths([
            evicted.clone(),
            evicted.clone(),
            evicted.clone(),
        ])
        .unwrap();

        assert_eq!(
            snapshot.entries,
            vec![FileProviderStateSnapshotEntry {
                path: evicted,
                state: CloudStorageState::Evicted,
            }]
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fileprovider_state_snapshot_write_rejects_duplicate_paths_before_persisting() {
        let root = unique_temp_dir();
        let path = root.join("fileprovider-state.tsv");
        let tracked = root.join("Remote.icloud-placeholder");
        let snapshot = FileProviderStateSnapshot {
            entries: vec![
                FileProviderStateSnapshotEntry {
                    path: tracked.clone(),
                    state: CloudStorageState::Evicted,
                },
                FileProviderStateSnapshotEntry {
                    path: tracked.clone(),
                    state: CloudStorageState::Downloaded,
                },
            ],
        };

        let err = snapshot.write(&path).unwrap_err();

        assert!(err
            .to_string()
            .contains("duplicate FileProvider state path"));
        assert!(err.to_string().contains("before writing"));
        assert!(err.to_string().contains(&tracked.display().to_string()));
        assert!(!path.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn initial_state_invalidation_skips_name_only_local_paths() {
        let root = unique_temp_dir();
        let local = root.join("Downloaded.icloud.md");
        fs::write(&local, "local").unwrap();

        let (report, snapshot) =
            FileProviderStateInvalidationReport::evaluate(None, [local]).unwrap();

        assert!(report.initialized);
        assert!(report.changes.is_empty());
        assert!(!report.invalidate_icon);
        assert!(!report.invalidate_preview_memory);
        assert!(!report.invalidate_preview_disk);
        assert!(!report.invalidate_sidebar);
        assert!(!report.reindex_metadata);
        assert!(snapshot.entries.is_empty());

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn state_invalidation_surfaces_path_probe_errors_before_removal_classification() {
        let path = PathBuf::from(OsString::from_vec(
            b"/tmp/gfm-fileprovider-invalidation-invalid\0path".to_vec(),
        ));

        let err = FileProviderStateInvalidationReport::evaluate(None, [path]).unwrap_err();

        assert!(err.to_string().contains("path existence unavailable"));
    }

    #[test]
    fn checked_state_invalidation_honors_pre_cancelled_control_before_path_probe() {
        let path = PathBuf::from("/tmp/gfm-fileprovider-state-invalidation-cancelled");

        let err = FileProviderStateInvalidationReport::evaluate_checked(None, [path], || {
            Err(GfmError::Cancelled)
        })
        .unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
    }

    #[test]
    fn state_invalidation_removes_tracked_entry_when_provider_evidence_disappears() {
        let root = unique_temp_dir();
        let local = root.join("Downloaded.icloud.md");
        fs::write(&local, "local").unwrap();
        let previous = FileProviderStateSnapshot {
            entries: vec![FileProviderStateSnapshotEntry {
                path: local.clone(),
                state: CloudStorageState::Downloaded,
            }],
        };

        let (report, snapshot) =
            FileProviderStateInvalidationReport::evaluate(Some(&previous), [local.clone()])
                .unwrap();

        assert!(!report.initialized);
        assert_eq!(report.changes.len(), 1);
        assert!(report.invalidate_icon);
        assert!(report.invalidate_preview_memory);
        assert!(report.invalidate_preview_disk);
        assert!(report.invalidate_sidebar);
        assert!(report.reindex_metadata);
        let change = &report.changes[0];
        assert_eq!(change.path, local);
        assert_eq!(change.previous, CloudStorageState::Downloaded);
        assert_eq!(change.current.storage_state, CloudStorageState::LocalOnly);
        assert_eq!(change.current.domain, FileProviderDomain::Local);
        assert_eq!(change.reason, "fileprovider-state-changed");
        assert!(snapshot.entries.is_empty());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fileprovider_state_snapshot_write_creates_parent_directory() {
        let root = unique_temp_dir();
        let path = root.join("runtime").join("fileprovider-state.tsv");
        let snapshot = FileProviderStateSnapshot {
            entries: vec![FileProviderStateSnapshotEntry {
                path: root.join("Remote.icloud-placeholder"),
                state: CloudStorageState::Evicted,
            }],
        };

        snapshot.write(&path).unwrap();

        assert_eq!(FileProviderStateSnapshot::read(&path).unwrap(), snapshot);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fileprovider_state_invalidation_persists_current_provider_transitions() {
        let root = unique_temp_dir();
        let evicted = root.join("Remote.icloud-placeholder");
        fs::write(&evicted, "placeholder").unwrap();
        mark_evicted_fixture(&evicted);
        let previous = FileProviderStateSnapshot {
            entries: vec![FileProviderStateSnapshotEntry {
                path: evicted.clone(),
                state: CloudStorageState::Downloaded,
            }],
        };

        let (report, snapshot) =
            FileProviderStateInvalidationReport::evaluate(Some(&previous), [evicted.clone()])
                .unwrap();

        assert!(!report.initialized);
        assert_eq!(report.changes.len(), 1);
        assert!(report.invalidate_icon);
        assert!(report.invalidate_preview_memory);
        assert!(report.invalidate_preview_disk);
        assert!(report.invalidate_sidebar);
        assert!(report.reindex_metadata);
        assert_eq!(snapshot.entries[0].path, evicted);
        assert_eq!(snapshot.entries[0].state, CloudStorageState::Evicted);
        assert!(report
            .as_tsv()
            .contains("previous=downloaded\tcurrent=evicted"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fileprovider_state_invalidation_deduplicates_current_paths_before_reads() {
        let root = unique_temp_dir();
        let evicted = root.join("Remote.icloud-placeholder");
        fs::write(&evicted, "placeholder").unwrap();
        mark_evicted_fixture(&evicted);
        let previous = FileProviderStateSnapshot {
            entries: vec![FileProviderStateSnapshotEntry {
                path: evicted.clone(),
                state: CloudStorageState::Downloaded,
            }],
        };

        let (report, snapshot) = FileProviderStateInvalidationReport::evaluate(
            Some(&previous),
            [evicted.clone(), evicted.clone(), evicted.clone()],
        )
        .unwrap();

        assert!(!report.initialized);
        assert_eq!(report.changes.len(), 1);
        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.entries[0].path, evicted);
        assert_eq!(snapshot.entries[0].state, CloudStorageState::Evicted);
        assert!(report.invalidate_icon);
        assert!(report.invalidate_preview_memory);
        assert!(report.invalidate_preview_disk);
        assert!(report.invalidate_sidebar);
        assert!(report.reindex_metadata);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn observed_fileprovider_invalidation_maps_events_to_provider_paths() {
        let root = unique_temp_dir();
        let evicted = root.join("Remote.icloud-placeholder");
        fs::write(&evicted, "placeholder").unwrap();
        mark_evicted_fixture(&evicted);
        let previous = FileProviderStateSnapshot {
            entries: vec![FileProviderStateSnapshotEntry {
                path: evicted.clone(),
                state: CloudStorageState::Downloaded,
            }],
        };
        let events = vec![FileEvent::new(&evicted, FileEventKind::Metadata)];

        let (observed, snapshot) =
            FileProviderObservedInvalidation::evaluate(Some(&previous), events).unwrap();

        assert_eq!(observed.events, 1);
        assert_eq!(
            observed.event_kinds,
            vec![FileProviderObservedEventKind::Metadata]
        );
        assert_eq!(observed.paths, vec![evicted.clone()]);
        assert_eq!(observed.report.changes.len(), 1);
        assert!(observed.report.invalidate_icon);
        assert!(observed.report.invalidate_preview_memory);
        assert!(observed.report.invalidate_preview_disk);
        assert!(observed.report.invalidate_sidebar);
        assert!(observed.report.reindex_metadata);
        assert_eq!(snapshot.entries[0].path, evicted);
        assert_eq!(snapshot.entries[0].state, CloudStorageState::Evicted);
        assert!(observed.as_tsv().contains(
            "fileprovider-observed-invalidation\tevents=1\tevent-kinds=metadata\tpaths=1"
        ));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn checked_observed_invalidation_honors_pre_cancelled_control_before_event_expansion() {
        let path = PathBuf::from("/tmp/gfm-fileprovider-observed-cancelled.icloud");
        let events = vec![FileEvent::new(&path, FileEventKind::Metadata)];

        let err = FileProviderObservedInvalidation::evaluate_checked(None, events, || {
            Err(GfmError::Cancelled)
        })
        .unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
    }

    #[test]
    fn checked_observed_invalidation_can_cancel_before_observed_hint_read() {
        let root = unique_temp_dir();
        let evicted = root.join("Remote.icloud-placeholder");
        fs::write(&evicted, "placeholder").unwrap();
        mark_evicted_fixture(&evicted);
        let events = vec![FileEvent::new(&evicted, FileEventKind::Metadata)];
        let mut checks = 0usize;

        let err = FileProviderObservedInvalidation::evaluate_checked(None, events, || {
            checks += 1;
            if checks >= 8 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        })
        .unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn observed_fileprovider_invalidation_coalesces_duplicate_event_paths() {
        let root = unique_temp_dir();
        let evicted = root.join("Remote.icloud-placeholder");
        fs::write(&evicted, "placeholder").unwrap();
        mark_evicted_fixture(&evicted);
        let previous = FileProviderStateSnapshot {
            entries: vec![FileProviderStateSnapshotEntry {
                path: evicted.clone(),
                state: CloudStorageState::Downloaded,
            }],
        };
        let events = vec![
            FileEvent::new(&evicted, FileEventKind::Metadata),
            FileEvent::new(&evicted, FileEventKind::Metadata),
            FileEvent::new(&evicted, FileEventKind::Metadata),
        ];

        let (observed, snapshot) =
            FileProviderObservedInvalidation::evaluate(Some(&previous), events).unwrap();

        assert_eq!(observed.events, 3);
        assert_eq!(observed.paths, vec![evicted.clone()]);
        assert_eq!(observed.report.changes.len(), 1);
        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.entries[0].path, evicted);
        assert_eq!(snapshot.entries[0].state, CloudStorageState::Evicted);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn observed_fileprovider_invalidation_reports_sorted_event_kinds() {
        let root = unique_temp_dir();
        let old = root.join("Old.icloud-placeholder");
        let new = root.join("New.icloud-placeholder");
        fs::write(&old, "placeholder").unwrap();
        fs::write(&new, "placeholder").unwrap();
        mark_evicted_fixture(&old);
        mark_evicted_fixture(&new);
        let previous = FileProviderStateSnapshot {
            entries: vec![FileProviderStateSnapshotEntry {
                path: old.clone(),
                state: CloudStorageState::Downloaded,
            }],
        };
        let events = vec![
            FileEvent::new(&new, FileEventKind::Metadata),
            FileEvent::new(
                &new,
                FileEventKind::Rename {
                    from: old.clone(),
                    to: new.clone(),
                },
            ),
        ];

        let (observed, snapshot) =
            FileProviderObservedInvalidation::evaluate(Some(&previous), events).unwrap();

        assert_eq!(
            observed.event_kinds,
            vec![
                FileProviderObservedEventKind::Metadata,
                FileProviderObservedEventKind::Rename,
            ]
        );
        assert_eq!(observed.paths, vec![new.clone(), old.clone()]);
        assert!(snapshot
            .entries
            .iter()
            .any(|entry| entry.path == new && entry.state == CloudStorageState::Evicted));
        assert!(observed.as_tsv().starts_with(
            "fileprovider-observed-invalidation\tevents=2\tevent-kinds=metadata,rename\tpaths=2\n"
        ));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn observed_fileprovider_invalidation_preserves_snapshot_for_irrelevant_events() {
        let root = unique_temp_dir();
        let tracked = root.join("Remote.icloud-placeholder");
        fs::write(&tracked, "placeholder").unwrap();
        mark_evicted_fixture(&tracked);
        let previous = FileProviderStateSnapshot {
            entries: vec![FileProviderStateSnapshotEntry {
                path: tracked.clone(),
                state: CloudStorageState::Evicted,
            }],
        };
        let events = vec![FileEvent::new(
            root.join("Missing.txt"),
            FileEventKind::Remove,
        )];

        let (observed, snapshot) =
            FileProviderObservedInvalidation::evaluate(Some(&previous), events).unwrap();

        assert_eq!(observed.events, 1);
        assert!(observed.paths.is_empty());
        assert!(observed.report.changes.is_empty());
        assert!(!observed.report.invalidate_icon);
        assert!(!observed.report.invalidate_preview_memory);
        assert!(!observed.report.invalidate_preview_disk);
        assert_eq!(snapshot, previous);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn observed_fileprovider_invalidation_ignores_existing_local_file_events() {
        let root = unique_temp_dir();
        let tracked = root.join("Remote.icloud-placeholder");
        let local = root.join("Notes.txt");
        fs::write(&tracked, "placeholder").unwrap();
        fs::write(&local, "ordinary local file").unwrap();
        let previous = FileProviderStateSnapshot {
            entries: vec![FileProviderStateSnapshotEntry {
                path: tracked.clone(),
                state: CloudStorageState::Evicted,
            }],
        };
        let events = vec![FileEvent::new(&local, FileEventKind::Modify)];

        let (observed, snapshot) =
            FileProviderObservedInvalidation::evaluate(Some(&previous), events).unwrap();

        assert_eq!(observed.events, 1);
        assert!(observed.paths.is_empty());
        assert!(observed.report.changes.is_empty());
        assert_eq!(snapshot, previous);
        assert!(!snapshot.entries.iter().any(|entry| entry.path == local));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn observed_fileprovider_invalidation_surfaces_path_probe_errors() {
        let root = unique_temp_dir();
        let path = root.join("Observed.icloud-placeholder".repeat(64));
        let events = vec![FileEvent::new(&path, FileEventKind::Metadata)];

        let err = FileProviderObservedInvalidation::evaluate(None, events).unwrap_err();

        assert!(err
            .to_string()
            .contains("observed path existence unavailable"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn weak_provider_name_hint_is_not_observable_when_native_proves_local_only() {
        let path = PathBuf::from("/tmp/icloud-meeting-notes.md");
        let mut native = native_values();
        native.is_ubiquitous = Some(false);
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "fixture-name+native-url-resource".to_string(),
        };

        assert!(!strong_provider_path_hint(&path));
        assert!(!observable_fileprovider_path_from_hints(&path, &hints));
    }

    #[test]
    fn weak_provider_name_hint_is_not_observable_without_provider_evidence() {
        let path = PathBuf::from("/tmp/Remote.fileprovider");
        let hints = CloudHints {
            native: native_values(),
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "fixture-name".to_string(),
        };

        assert!(!strong_provider_path_hint(&path));
        assert!(weak_path_hint_without_provider_evidence(&hints));
        assert!(!observable_fileprovider_path_from_hints(&path, &hints));
    }

    #[test]
    fn evicted_icloud_extension_remains_strong_observable_provider_hint() {
        let path = PathBuf::from("/tmp/Remote.icloud");

        assert!(strong_provider_path_hint(&path));
    }

    #[test]
    fn observable_strong_provider_hint_is_suppressed_by_native_local_false() {
        let path = PathBuf::from("/tmp/Remote.icloud");
        let mut native = native_values();
        native.is_ubiquitous = Some(false);
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "icloud-extension+native-url-resource".to_string(),
        };

        assert!(strong_provider_path_hint(&path));
        assert!(!observable_fileprovider_path_from_hints(&path, &hints));
    }

    #[test]
    fn observed_fileprovider_invalidation_removes_deleted_tracked_provider_item() {
        let root = unique_temp_dir();
        let tracked = root.join("Downloaded.icloud.md");
        fs::write(&tracked, "downloaded").unwrap();
        let previous = FileProviderStateSnapshot {
            entries: vec![FileProviderStateSnapshotEntry {
                path: tracked.clone(),
                state: CloudStorageState::Downloaded,
            }],
        };
        fs::remove_file(&tracked).unwrap();
        let events = vec![FileEvent::new(&tracked, FileEventKind::Remove)];

        let (observed, snapshot) =
            FileProviderObservedInvalidation::evaluate(Some(&previous), events).unwrap();

        assert_eq!(observed.events, 1);
        assert_eq!(observed.paths, vec![tracked.clone()]);
        assert!(snapshot.entries.is_empty());
        assert_eq!(observed.report.changes.len(), 1);
        assert!(observed.report.invalidate_icon);
        assert!(observed.report.invalidate_preview_memory);
        assert!(observed.report.invalidate_preview_disk);
        assert!(observed.report.invalidate_sidebar);
        assert!(observed.report.reindex_metadata);
        let change = &observed.report.changes[0];
        assert_eq!(change.previous, CloudStorageState::Downloaded);
        assert_eq!(change.current.storage_state, CloudStorageState::Removed);
        assert_eq!(change.current.domain, FileProviderDomain::ICloudDrive);
        assert_eq!(
            change.current.materialization,
            CloudMaterialization::Unknown
        );
        assert_eq!(
            change.current.materialization_source,
            CloudMaterializationSource::StateFallback
        );
        assert!(change.current.offline);
        assert!(change.current.badges.is_empty());
        assert_eq!(change.current.source, "removed");
        assert_eq!(
            change.current.progress.reason.as_deref(),
            Some("fileprovider-item-removed")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn observed_fileprovider_invalidation_removes_deleted_tracked_provider_subtree() {
        let root = unique_temp_dir();
        let removed_dir = root.join("Removed.icloud");
        let removed_child = removed_dir.join("Child.icloud.md");
        let untouched = root.join("Untouched.icloud-placeholder");
        fs::create_dir_all(&removed_dir).unwrap();
        fs::write(&removed_child, "downloaded").unwrap();
        fs::write(&untouched, "placeholder").unwrap();
        mark_evicted_fixture(&untouched);
        let previous = FileProviderStateSnapshot {
            entries: vec![
                FileProviderStateSnapshotEntry {
                    path: removed_child.clone(),
                    state: CloudStorageState::Downloaded,
                },
                FileProviderStateSnapshotEntry {
                    path: untouched.clone(),
                    state: CloudStorageState::Evicted,
                },
            ],
        };
        fs::remove_dir_all(&removed_dir).unwrap();
        let events = vec![FileEvent::new(&removed_dir, FileEventKind::Remove)];

        let (observed, snapshot) =
            FileProviderObservedInvalidation::evaluate(Some(&previous), events).unwrap();

        assert_eq!(observed.events, 1);
        assert_eq!(observed.paths, vec![removed_child.clone()]);
        assert_eq!(observed.report.changes.len(), 1);
        let change = &observed.report.changes[0];
        assert_eq!(change.path, removed_child);
        assert_eq!(change.previous, CloudStorageState::Downloaded);
        assert_eq!(change.current.storage_state, CloudStorageState::Removed);
        assert!(observed.report.invalidate_preview_disk);
        assert!(observed.report.invalidate_sidebar);
        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.entries[0].path, untouched);
        assert_eq!(snapshot.entries[0].state, CloudStorageState::Evicted);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn observed_fileprovider_invalidation_moves_tracked_provider_subtree_on_rename() {
        let root = unique_temp_dir();
        let old_dir = root.join("Old.icloud");
        let new_dir = root.join("New.icloud");
        let old_child = old_dir.join("Child.icloud-placeholder");
        let new_child = new_dir.join("Child.icloud-placeholder");
        fs::create_dir_all(&old_dir).unwrap();
        fs::write(&old_child, "placeholder").unwrap();
        mark_evicted_fixture(&old_child);
        let previous = FileProviderStateSnapshot {
            entries: vec![FileProviderStateSnapshotEntry {
                path: old_child.clone(),
                state: CloudStorageState::Downloaded,
            }],
        };
        fs::rename(&old_dir, &new_dir).unwrap();
        let events = vec![FileEvent::new(
            &new_dir,
            FileEventKind::Rename {
                from: old_dir.clone(),
                to: new_dir.clone(),
            },
        )];

        let (observed, snapshot) =
            FileProviderObservedInvalidation::evaluate(Some(&previous), events).unwrap();

        assert_eq!(observed.events, 1);
        assert_eq!(
            observed.paths,
            vec![new_dir.clone(), new_child.clone(), old_child.clone()]
        );
        assert!(observed.report.changes.iter().any(|change| {
            change.path == old_child
                && change.previous == CloudStorageState::Downloaded
                && change.current.storage_state == CloudStorageState::Removed
        }));
        assert!(observed
            .report
            .changes
            .iter()
            .any(|change| change.path == new_child
                && change.previous == CloudStorageState::LocalOnly
                && change.current.storage_state == CloudStorageState::Evicted));
        assert!(!snapshot.entries.iter().any(|entry| entry.path == old_child));
        assert!(snapshot
            .entries
            .iter()
            .any(|entry| entry.path == new_child && entry.state == CloudStorageState::Evicted));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn observed_fileprovider_invalidation_preserves_unrelated_snapshot_entries() {
        let root = unique_temp_dir();
        let changed = root.join("Changed.icloud-placeholder");
        let untouched = root.join("Untouched.icloud-placeholder");
        fs::write(&changed, "placeholder").unwrap();
        fs::write(&untouched, "placeholder").unwrap();
        mark_evicted_fixture(&changed);
        mark_evicted_fixture(&untouched);
        let previous = FileProviderStateSnapshot {
            entries: vec![
                FileProviderStateSnapshotEntry {
                    path: changed.clone(),
                    state: CloudStorageState::Downloaded,
                },
                FileProviderStateSnapshotEntry {
                    path: untouched.clone(),
                    state: CloudStorageState::Evicted,
                },
            ],
        };
        let events = vec![FileEvent::new(&changed, FileEventKind::Metadata)];

        let (observed, snapshot) =
            FileProviderObservedInvalidation::evaluate(Some(&previous), events).unwrap();

        assert_eq!(observed.paths, vec![changed.clone()]);
        assert_eq!(observed.report.changes.len(), 1);
        assert_eq!(snapshot.entries.len(), 2);
        assert!(snapshot
            .entries
            .iter()
            .any(|entry| entry.path == changed && entry.state == CloudStorageState::Evicted));
        assert!(snapshot
            .entries
            .iter()
            .any(|entry| entry.path == untouched && entry.state == CloudStorageState::Evicted));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn observed_snapshot_merge_filters_exact_paths_and_descendants() {
        let root = PathBuf::from("/tmp/gfm-merge-observed-snapshot");
        let exact = root.join("Exact.icloud-placeholder");
        let subtree = root.join("Folder.icloud");
        let child = subtree.join("Child.icloud-placeholder");
        let unrelated = root.join("Unrelated.icloud-placeholder");
        let replacement = root.join("Replacement.icloud-placeholder");
        let previous = FileProviderStateSnapshot {
            entries: vec![
                FileProviderStateSnapshotEntry {
                    path: exact.clone(),
                    state: CloudStorageState::Downloaded,
                },
                FileProviderStateSnapshotEntry {
                    path: child,
                    state: CloudStorageState::Evicted,
                },
                FileProviderStateSnapshotEntry {
                    path: unrelated.clone(),
                    state: CloudStorageState::Evicted,
                },
            ],
        };
        let event_snapshot = FileProviderStateSnapshot {
            entries: vec![FileProviderStateSnapshotEntry {
                path: replacement.clone(),
                state: CloudStorageState::Downloaded,
            }],
        };

        let merged =
            merge_observed_snapshot(Some(&previous), &[exact, subtree], event_snapshot).unwrap();

        assert_eq!(
            merged.entries,
            vec![
                FileProviderStateSnapshotEntry {
                    path: replacement,
                    state: CloudStorageState::Downloaded,
                },
                FileProviderStateSnapshotEntry {
                    path: unrelated,
                    state: CloudStorageState::Evicted,
                },
            ]
        );
    }

    #[test]
    fn observed_snapshot_merge_rejects_duplicate_merged_paths() {
        let root = PathBuf::from("/tmp/gfm-merge-observed-snapshot-duplicate");
        let unrelated = root.join("Unrelated.icloud-placeholder");
        let observed = root.join("Observed.icloud-placeholder");
        let previous = FileProviderStateSnapshot {
            entries: vec![FileProviderStateSnapshotEntry {
                path: unrelated.clone(),
                state: CloudStorageState::Evicted,
            }],
        };
        let event_snapshot = FileProviderStateSnapshot {
            entries: vec![FileProviderStateSnapshotEntry {
                path: unrelated.clone(),
                state: CloudStorageState::Downloaded,
            }],
        };

        let err =
            merge_observed_snapshot(Some(&previous), &[observed], event_snapshot).unwrap_err();

        assert!(err
            .to_string()
            .contains("duplicate FileProvider state path"));
        assert!(err
            .to_string()
            .contains("merged observed FileProvider snapshot"));
        assert!(err.to_string().contains(&unrelated.display().to_string()));
    }

    #[test]
    fn observed_fileprovider_invalidation_delete_preserves_unrelated_snapshot_entries() {
        let root = unique_temp_dir();
        let removed = root.join("Removed.icloud.md");
        let untouched = root.join("Untouched.icloud-placeholder");
        fs::write(&removed, "downloaded").unwrap();
        fs::write(&untouched, "placeholder").unwrap();
        let previous = FileProviderStateSnapshot {
            entries: vec![
                FileProviderStateSnapshotEntry {
                    path: removed.clone(),
                    state: CloudStorageState::Downloaded,
                },
                FileProviderStateSnapshotEntry {
                    path: untouched.clone(),
                    state: CloudStorageState::Evicted,
                },
            ],
        };
        fs::remove_file(&removed).unwrap();
        let events = vec![FileEvent::new(&removed, FileEventKind::Remove)];

        let (observed, snapshot) =
            FileProviderObservedInvalidation::evaluate(Some(&previous), events).unwrap();

        assert_eq!(observed.paths, vec![removed.clone()]);
        assert_eq!(observed.report.changes.len(), 1);
        let change = &observed.report.changes[0];
        assert_eq!(change.previous, CloudStorageState::Downloaded);
        assert_eq!(change.current.storage_state, CloudStorageState::Removed);
        assert_eq!(change.current.domain, FileProviderDomain::ICloudDrive);
        assert_eq!(
            change.current.materialization_reason.as_deref(),
            Some("fileprovider-item-removed")
        );
        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.entries[0].path, untouched);
        assert_eq!(snapshot.entries[0].state, CloudStorageState::Evicted);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn state_read_does_not_query_native_manager_identity_on_hot_path() {
        let root = unique_temp_dir();
        let path = root.join("Downloaded.icloud.md");
        fs::write(&path, "downloaded").unwrap();

        let hints = CloudHints::read(&path);
        let report = FileProviderStateReport::read_path(&path).unwrap();

        assert_eq!(
            hints.native_identity.status,
            NativeFileProviderIdentityStatus::NotQueried
        );
        assert!(!report
            .source
            .split('+')
            .any(|source| source == "nsfileprovidermanager"));
        assert_eq!(report.domain, FileProviderDomain::Local);
        assert_eq!(report.storage_state, CloudStorageState::LocalOnly);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_url_resource_false_keeps_path_hints_local_only() {
        let root = unique_temp_dir();
        let path = root.join("Downloaded.icloud.md");
        fs::write(&path, "downloaded").unwrap();
        let mut native = native_values();
        native.is_ubiquitous = Some(false);
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "fixture-name+native-url-resource".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path.clone(), hints);

        assert_eq!(report.domain, FileProviderDomain::Local);
        assert_eq!(report.storage_state, CloudStorageState::LocalOnly);
        assert_eq!(
            report.materialization,
            CloudMaterialization::NotProviderBacked
        );
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::NativeUrlResource
        );
        assert_eq!(
            report.materialization_reason.as_deref(),
            Some("native-url-resource-not-provider-backed")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_provider_metadata_overrides_native_local_false() {
        let root = unique_temp_dir();
        let path = root.join("Remote.icloud.md");
        fs::write(&path, "downloaded").unwrap();
        let mut native = native_values();
        native.is_ubiquitous = Some(false);
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::Available,
                item_identifier: Some("item-123".to_string()),
                domain_identifier: Some("com.apple.CloudDocs".to_string()),
                reason: None,
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: Some("com.apple.CloudDocs".to_string()),
            source: "fixture-name+native-url-resource+nsfileprovidermanager".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path.clone(), hints);

        assert_eq!(report.domain, FileProviderDomain::ICloudDrive);
        assert_eq!(report.storage_state, CloudStorageState::Unknown);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::NativeFileProviderIdentityUnknown
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn domain_report_does_not_claim_local_files_are_provider_backed() {
        let root = unique_temp_dir();
        let path = root.join("Local.md");
        fs::write(&path, "local").unwrap();

        let report = FileProviderDomainReport::read_path(&path).unwrap();

        assert_eq!(report.domain, FileProviderDomain::Local);
        assert_eq!(
            report.native_identity_status,
            NativeFileProviderIdentityStatus::NotQueried
        );
        assert!(report.item_identifier.is_none());
        assert!(report.domain_identifier.is_none());
        assert_eq!(
            report.reason.as_deref(),
            Some("nsfileprovidermanager-identity-not-queried-on-hot-path")
        );
        assert!(report.as_tsv().starts_with("fileprovider-domain\t"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_identity_lookup_is_gated_to_provider_evidence() {
        let local = PathBuf::from("/tmp/Local.md");
        let mut hints = CloudHints {
            native: native_values(),
            native_identity: identity_not_queried(),
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "filesystem".to_string(),
        };

        assert!(!should_query_native_fileprovider_identity(&local, &hints));

        let explicit_icloud_extension = PathBuf::from("/tmp/Remote.icloud");
        assert!(should_query_native_fileprovider_identity(
            &explicit_icloud_extension,
            &hints
        ));

        hints
            .xattrs
            .push("com.apple.fileprovider.domain".to_string());
        assert!(should_query_native_fileprovider_identity(&local, &hints));

        hints.xattrs.clear();
        hints.provider_identifier = Some("com.example.drive".to_string());
        assert!(should_query_native_fileprovider_identity(&local, &hints));

        hints.provider_identifier = None;
        hints.native.is_ubiquitous = Some(true);
        assert!(should_query_native_fileprovider_identity(&local, &hints));

        hints.native.is_ubiquitous = Some(false);
        assert!(!should_query_native_fileprovider_identity(&local, &hints));
    }

    #[cfg(unix)]
    #[test]
    fn domain_report_surfaces_path_probe_errors_before_provider_hints() {
        let path = PathBuf::from(OsString::from_vec(
            b"/tmp/gfm-fileprovider-domain-invalid\0path".to_vec(),
        ));

        let err = FileProviderDomainReport::read_path(&path).unwrap_err();

        assert!(err.to_string().contains("path existence unavailable"));
    }

    #[cfg(unix)]
    #[test]
    fn state_report_surfaces_path_probe_errors_before_provider_hints() {
        let path = PathBuf::from(OsString::from_vec(
            b"/tmp/gfm-fileprovider-state-invalid\0path".to_vec(),
        ));

        let err = FileProviderStateReport::read_path(&path).unwrap_err();

        assert!(err.to_string().contains("path existence unavailable"));
    }

    #[cfg(unix)]
    #[test]
    fn checked_state_report_honors_pre_cancelled_work_before_path_probe() {
        let path = PathBuf::from(OsString::from_vec(
            b"/tmp/gfm-fileprovider-state-cancelled\0path".to_vec(),
        ));

        let err = FileProviderStateReport::read_path_checked(&path, || Err(GfmError::Cancelled))
            .unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
    }

    #[cfg(unix)]
    #[test]
    fn checked_domain_report_honors_pre_cancelled_work_before_path_probe() {
        let path = PathBuf::from(OsString::from_vec(
            b"/tmp/gfm-fileprovider-domain-cancelled\0path".to_vec(),
        ));

        let err = FileProviderDomainReport::read_path_checked(&path, || Err(GfmError::Cancelled))
            .unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
    }

    #[cfg(unix)]
    #[test]
    fn checked_conflict_report_honors_pre_cancelled_work_before_path_probe() {
        let path = PathBuf::from(OsString::from_vec(
            b"/tmp/gfm-fileprovider-conflict-cancelled\0path".to_vec(),
        ));

        let err = FileProviderConflictReport::read_path_checked(&path, || Err(GfmError::Cancelled))
            .unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
    }

    #[cfg(unix)]
    #[test]
    fn checked_invalidation_report_honors_pre_cancelled_work_before_path_probe() {
        let path = PathBuf::from(OsString::from_vec(
            b"/tmp/gfm-fileprovider-invalidation-cancelled\0path".to_vec(),
        ));

        let err = FileProviderInvalidationReport::evaluate_checked(
            &path,
            CloudStorageState::Downloaded,
            || Err(GfmError::Cancelled),
        )
        .unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
    }

    #[test]
    fn checked_state_report_can_cancel_during_hint_collection() {
        let root = unique_temp_dir();
        let path = root.join("Remote.icloud");
        fs::write(&path, "remote").unwrap();
        let mut checks = 0usize;

        let err = FileProviderStateReport::read_path_checked(&path, || {
            checks += 1;
            if checks >= 4 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        })
        .unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_manager_identity_marks_third_party_fileprovider_domain() {
        let path = PathBuf::from("/tmp/LocalName.txt");
        let hints = CloudHints {
            native: native_values(),
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::Available,
                item_identifier: Some("item-123".to_string()),
                domain_identifier: Some("com.example.drive.account".to_string()),
                reason: None,
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "nsfileprovidermanager".to_string(),
        };

        let report = FileProviderDomainReport::from_hints_and_domains(
            path,
            hints,
            NativeFileProviderDomainEnumeration {
                status: NativeFileProviderDomainStatus::Available,
                domains: vec![NativeFileProviderDomain {
                    identifier: Some("com.example.drive.account".to_string()),
                    display_name: Some("Example Drive".to_string()),
                    path_relative_to_document_storage: Some("Example Drive".to_string()),
                    disconnected: Some(false),
                }],
                reason: None,
            },
        );

        assert_eq!(report.domain, FileProviderDomain::FileProvider);
        assert_eq!(
            report.native_identity_status,
            NativeFileProviderIdentityStatus::Available
        );
        assert_eq!(
            report.native_manager_status,
            NativeFileProviderDomainStatus::Available
        );
        assert_eq!(report.item_identifier.as_deref(), Some("item-123"));
        assert_eq!(
            report.domain_identifier.as_deref(),
            Some("com.example.drive.account")
        );
        assert_eq!(
            report.matched_domain_display_name.as_deref(),
            Some("Example Drive")
        );
        assert_eq!(report.matched_domain_disconnected, Some(false));
        assert!(report.as_tsv().contains("\tdomain=fileprovider\t"));
        assert!(report
            .as_tsv()
            .contains("\tmatched-display=Example Drive\t"));
    }

    #[test]
    fn domain_enumeration_maps_native_domains_without_path_heuristics() {
        let report =
            FileProviderDomainEnumerationReport::from_native(NativeFileProviderDomainEnumeration {
                status: NativeFileProviderDomainStatus::Available,
                domains: vec![
                    NativeFileProviderDomain {
                        identifier: Some("com.apple.CloudDocs".to_string()),
                        display_name: Some("iCloud Drive".to_string()),
                        path_relative_to_document_storage: Some("Documents".to_string()),
                        disconnected: Some(false),
                    },
                    NativeFileProviderDomain {
                        identifier: Some("com.example.drive.account".to_string()),
                        display_name: Some("Example Drive".to_string()),
                        path_relative_to_document_storage: Some("Root".to_string()),
                        disconnected: Some(true),
                    },
                ],
                reason: None,
            });

        assert_eq!(report.status, NativeFileProviderDomainStatus::Available);
        assert_eq!(report.domains.len(), 2);
        assert_eq!(report.domains[0].domain, FileProviderDomain::ICloudDrive);
        assert_eq!(report.domains[1].domain, FileProviderDomain::FileProvider);
        let tsv = report.as_tsv();
        assert!(tsv.starts_with("fileprovider-domains\tstatus=available\tcount=2\t"));
        assert!(tsv.contains(
            "domain\tkind=icloud-drive\tidentifier=com.apple.CloudDocs\tdisplay-name=iCloud Drive"
        ));
        assert!(tsv.contains(
            "domain\tkind=fileprovider\tidentifier=com.example.drive.account\tdisplay-name=Example Drive"
        ));
    }

    #[test]
    fn icloud_domain_classifier_requires_cloud_specific_identity() {
        assert!(is_icloud_domain_identifier("com.apple.CloudDocs"));
        assert!(is_icloud_domain_identifier("TEAMID.com.vendor.ubiquity"));
        assert!(!is_icloud_domain_identifier("com.apple.finder.sync"));
        assert!(!is_icloud_domain_identifier("com.example.drive.account"));
    }

    #[test]
    fn provider_identity_without_materialization_evidence_is_unknown() {
        let root = unique_temp_dir();
        let path = root.join("ProviderItem.txt");
        fs::write(&path, "provider").unwrap();
        let hints = CloudHints {
            native: native_values(),
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::Available,
                item_identifier: Some("item-456".to_string()),
                domain_identifier: Some("com.example.drive.account".to_string()),
                reason: None,
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: Some("com.example.drive.account".to_string()),
            source: "nsfileprovidermanager".to_string(),
        };

        let domain = domain_for_path(&path, &hints);
        let state = storage_state_for_path(&path, domain, &hints);
        let progress = progress_for_state(state, &hints);

        assert_eq!(domain, FileProviderDomain::FileProvider);
        assert_eq!(state, CloudStorageState::Unknown);
        assert_eq!(progress.reason.as_deref(), Some("unknown-provider-state"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn storage_state_fallback_keeps_unavailable_path_unknown() {
        let root = unique_temp_dir();
        let path = root.join(format!(
            "{}.icloud.md",
            "downloaded-path-unavailable".repeat(16)
        ));
        let hints = CloudHints {
            native: native_values(),
            native_identity: identity_not_queried(),
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "filesystem".to_string(),
        };

        let state = storage_state_for_path(&path, FileProviderDomain::ICloudDrive, &hints);

        assert_eq!(state, CloudStorageState::Unknown);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn generic_provider_name_without_materialization_evidence_is_local() {
        let root = unique_temp_dir();
        let path = root.join("Remote.fileprovider");
        fs::write(&path, "provider").unwrap();
        let hints = CloudHints {
            native: native_values(),
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "fixture-name".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path.clone(), hints);

        assert_eq!(report.domain, FileProviderDomain::Local);
        assert_eq!(report.storage_state, CloudStorageState::LocalOnly);
        assert_eq!(
            report.materialization,
            CloudMaterialization::NotProviderBacked
        );
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::Filesystem
        );
        assert_eq!(
            report.commands.reason.as_deref(),
            Some("not-fileprovider-backed")
        );
        assert_eq!(
            report.progress.reason.as_deref(),
            Some("not-fileprovider-backed")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operations_refuse_fixture_only_provider_items() {
        let root = unique_temp_dir();
        let evicted = root.join("Evicted.icloud-placeholder");
        let downloaded = root.join("Downloaded.icloud.md");
        fs::write(&evicted, "placeholder").unwrap();
        fs::write(&downloaded, "downloaded").unwrap();

        let download =
            FileProviderOperationReport::execute(&evicted, FileProviderOperation::Download)
                .unwrap();
        assert_eq!(
            download.disposition,
            FileProviderOperationDisposition::Refused
        );
        assert_eq!(
            download.reason.as_deref(),
            Some("operation-disabled-for-current-state")
        );
        assert_eq!(download.before.storage_state, CloudStorageState::LocalOnly);

        let evict = FileProviderOperationReport::execute(&downloaded, FileProviderOperation::Evict)
            .unwrap();
        assert_eq!(evict.disposition, FileProviderOperationDisposition::Refused);
        assert_eq!(
            evict.reason.as_deref(),
            Some("operation-disabled-for-current-state")
        );
        assert_eq!(evict.before.storage_state, CloudStorageState::LocalOnly);

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn checked_operation_honors_pre_cancelled_work_before_path_probe() {
        let path = PathBuf::from(OsString::from_vec(
            b"/tmp/gfm-fileprovider-operation-cancelled\0path".to_vec(),
        ));

        let err = FileProviderOperationReport::execute_checked(
            &path,
            FileProviderOperation::Download,
            || Err(GfmError::Cancelled),
        )
        .expect_err("pre-cancelled FileProvider operation must stop before probing path");

        assert_eq!(err, GfmError::Cancelled);
    }

    #[test]
    fn operations_report_missing_placeholder_path_before_native_call() {
        let root = unique_temp_dir();
        let missing = root.join("Missing.icloud");

        let report =
            FileProviderOperationReport::execute(&missing, FileProviderOperation::Download)
                .unwrap();

        assert_eq!(
            report.disposition,
            FileProviderOperationDisposition::Missing
        );
        assert_eq!(report.reason.as_deref(), Some("fileprovider-path-missing"));
        assert_eq!(report.before.storage_state, CloudStorageState::Evicted);
        assert!(report.as_tsv().contains("\tdisposition=missing\t"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_fileprovider_permission_failure_maps_to_denied_disposition() {
        assert_eq!(
            disposition_for_native_fileprovider_operation(
                NativeFileProviderOperationStatus::PermissionDenied
            ),
            FileProviderOperationDisposition::Denied
        );
        assert_eq!(
            disposition_for_native_fileprovider_operation(
                NativeFileProviderOperationStatus::Unavailable
            ),
            FileProviderOperationDisposition::Unavailable
        );
        assert_eq!(
            disposition_for_native_fileprovider_operation(
                NativeFileProviderOperationStatus::Failed
            ),
            FileProviderOperationDisposition::Failed
        );
        assert_eq!(
            disposition_for_native_fileprovider_operation(
                NativeFileProviderOperationStatus::Cancelled
            ),
            FileProviderOperationDisposition::Cancelled
        );
        assert_eq!(
            FileProviderOperationDisposition::Cancelled.as_str(),
            "cancelled"
        );
        assert_eq!(
            disposition_for_native_fileprovider_operation(
                NativeFileProviderOperationStatus::Missing
            ),
            FileProviderOperationDisposition::Missing
        );
        assert_eq!(
            disposition_for_native_fileprovider_operation(
                NativeFileProviderOperationStatus::UnsupportedPath
            ),
            FileProviderOperationDisposition::Unsupported
        );
    }

    #[test]
    fn operations_refuse_disabled_state_before_native_call() {
        let root = unique_temp_dir();
        let downloading = root.join("Downloading.icloud-downloading.md");
        fs::write(&downloading, "downloading").unwrap();
        xattr::set(&downloading, "com.apple.fileprovider.state", b"downloading").unwrap();

        let report =
            FileProviderOperationReport::execute(&downloading, FileProviderOperation::Download)
                .unwrap();

        assert_eq!(
            report.disposition,
            FileProviderOperationDisposition::Refused
        );
        assert_eq!(
            report.reason.as_deref(),
            Some("operation-disabled-for-current-state")
        );
        assert_eq!(report.before.storage_state, CloudStorageState::Downloading);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operations_refuse_unresolved_provider_conflicts_before_native_call() {
        let root = unique_temp_dir();
        let conflict = root.join("Conflict.icloud-conflict.md");
        fs::write(&conflict, "conflict").unwrap();
        xattr::set(&conflict, "com.apple.fileprovider.state", b"conflict").unwrap();

        let report =
            FileProviderOperationReport::execute(&conflict, FileProviderOperation::Evict).unwrap();

        assert_eq!(
            report.disposition,
            FileProviderOperationDisposition::Refused
        );
        assert_eq!(
            report.reason.as_deref(),
            Some("provider-conflict-requires-resolution")
        );
        assert_eq!(report.before.storage_state, CloudStorageState::Conflict);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn progress_reports_indeterminate_in_fallback_in_flight_states() {
        let root = unique_temp_dir();
        let downloading = root.join("Downloading.icloud-downloading.md");
        fs::write(&downloading, "downloading").unwrap();
        xattr::set(&downloading, "com.apple.fileprovider.state", b"downloading").unwrap();

        let report = FileProviderProgressReport::read_path(&downloading).unwrap();

        assert_eq!(report.state.storage_state, CloudStorageState::Downloading);
        assert_eq!(
            report.state.progress.direction,
            CloudTransferDirection::Download
        );
        assert_eq!(report.state.progress.percent_milli, None);
        assert!(report.state.progress.requested);
        assert!(report.state.progress.indeterminate);
        assert_eq!(
            report.state.progress.reason.as_deref(),
            Some("provider-progress-unavailable")
        );
        assert!(report
            .as_tsv()
            .contains("\tprogress-direction=download\tprogress-milli=-\t"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn checked_progress_report_honors_pre_cancelled_work_before_path_probe() {
        let root = unique_temp_dir();
        let downloading = root.join("Downloading.icloud-downloading.md");
        fs::write(&downloading, "downloading").unwrap();

        let err = FileProviderProgressReport::read_path_checked(&downloading, || {
            Err(GfmError::Cancelled)
        })
        .unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn checked_progress_report_can_cancel_during_state_read() {
        let root = unique_temp_dir();
        let downloading = root.join("Downloading.icloud-downloading.md");
        fs::write(&downloading, "downloading").unwrap();
        xattr::set(&downloading, "com.apple.fileprovider.state", b"downloading").unwrap();
        let mut checks = 0usize;

        let err = FileProviderProgressReport::read_path_checked(&downloading, || {
            checks += 1;
            if checks >= 4 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        })
        .unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalidation_marks_provider_state_transitions() {
        let root = unique_temp_dir();
        let evicted = root.join("Remote.icloud-placeholder");
        fs::write(&evicted, "placeholder").unwrap();
        mark_evicted_fixture(&evicted);

        let report =
            FileProviderInvalidationReport::evaluate(&evicted, CloudStorageState::Downloaded)
                .unwrap();

        assert!(report.state_changed);
        assert!(report.invalidate_icon);
        assert!(report.invalidate_preview_memory);
        assert!(report.invalidate_preview_disk);
        assert!(report.invalidate_sidebar);
        assert!(report.reindex_metadata);
        assert_eq!(report.reason, "fileprovider-state-changed");
        assert_eq!(report.current.storage_state, CloudStorageState::Evicted);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalidation_does_not_churn_unchanged_provider_state() {
        let root = unique_temp_dir();
        let downloaded = root.join("Downloaded.icloud.md");
        fs::write(&downloaded, "downloaded").unwrap();

        let report =
            FileProviderInvalidationReport::evaluate(&downloaded, CloudStorageState::LocalOnly)
                .unwrap();

        assert!(!report.state_changed);
        assert!(!report.invalidate_icon);
        assert!(!report.invalidate_preview_memory);
        assert!(!report.invalidate_preview_disk);
        assert!(!report.invalidate_sidebar);
        assert!(!report.reindex_metadata);
        assert_eq!(report.reason, "not-provider-visible");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn state_snapshot_round_trips_sorted_provider_states() {
        let root = unique_temp_dir();
        let snapshot_path = root.join("state.tsv");
        let first = root.join("B.icloud-placeholder");
        let second = root.join("A.icloud-downloaded");
        let snapshot = FileProviderStateSnapshot {
            entries: vec![
                FileProviderStateSnapshotEntry {
                    path: first.clone(),
                    state: CloudStorageState::Evicted,
                },
                FileProviderStateSnapshotEntry {
                    path: second.clone(),
                    state: CloudStorageState::Downloaded,
                },
            ],
        };

        snapshot.write(&snapshot_path).unwrap();
        let text = fs::read_to_string(&snapshot_path).unwrap();
        assert!(text.starts_with("gfm-fileprovider-state-v1\n"));
        assert!(
            text.find("A.icloud-downloaded").unwrap() < text.find("B.icloud-placeholder").unwrap()
        );
        let restored = FileProviderStateSnapshot::read(&snapshot_path).unwrap();

        assert_eq!(restored.entries.len(), 2);
        assert_eq!(restored.entries[0].path, second);
        assert_eq!(restored.entries[0].state, CloudStorageState::Downloaded);
        assert_eq!(restored.entries[1].path, first);
        assert_eq!(restored.entries[1].state, CloudStorageState::Evicted);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn checked_state_snapshot_write_preserves_existing_state_on_cancel() {
        let root = unique_temp_dir();
        let snapshot_path = root.join("state.tsv");
        let previous = FileProviderStateSnapshot {
            entries: vec![FileProviderStateSnapshotEntry {
                path: root.join("Remote.icloud-placeholder"),
                state: CloudStorageState::Evicted,
            }],
        };
        let current = FileProviderStateSnapshot {
            entries: vec![FileProviderStateSnapshotEntry {
                path: root.join("Remote.icloud-downloaded"),
                state: CloudStorageState::Downloaded,
            }],
        };
        previous.write(&snapshot_path).unwrap();
        let before = fs::read(&snapshot_path).unwrap();
        let mut checks = 0usize;

        let err = current
            .write_checked(&snapshot_path, || {
                checks += 1;
                if checks >= 3 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            })
            .unwrap_err();

        assert_eq!(err, GfmError::Cancelled);
        assert_eq!(fs::read(&snapshot_path).unwrap(), before);
        let leftovers = fs::read_dir(&root)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".state.tsv")
            })
            .count();
        assert_eq!(leftovers, 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn state_snapshot_writes_relative_leaf_state_file_in_current_directory() {
        let _cwd = CWD_LOCK.lock().unwrap();
        let root = unique_temp_dir();
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(&root).unwrap();
        let snapshot = FileProviderStateSnapshot {
            entries: vec![FileProviderStateSnapshotEntry {
                path: PathBuf::from("Remote.icloud"),
                state: CloudStorageState::Evicted,
            }],
        };

        snapshot.write("fileprovider-state.tsv").unwrap();
        let restored = FileProviderStateSnapshot::read("fileprovider-state.tsv").unwrap();

        assert_eq!(restored.entries, snapshot.entries);
        assert!(root.join("fileprovider-state.tsv").exists());
        std::env::set_current_dir(previous).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parses_cloud_storage_state_for_operator_surfaces() {
        assert_eq!(
            CloudStorageState::parse("downloading").unwrap(),
            CloudStorageState::Downloading
        );
        assert_eq!(
            CloudStorageState::parse("removed").unwrap(),
            CloudStorageState::Removed
        );
        assert!(CloudStorageState::parse("not-real").is_err());
    }

    #[test]
    fn native_ubiquitous_downloading_state_overrides_fixture_name() {
        let path = PathBuf::from("/tmp/Downloaded.icloud.md");
        let mut native = native_values();
        native.is_ubiquitous = Some(true);
        native.has_unresolved_conflicts = Some(false);
        native.is_downloading = Some(true);
        native.is_uploading = Some(false);
        native.is_uploaded = Some(true);
        native.download_requested = Some(true);
        native.percent_downloaded_milli = Some(12_500);
        native.percent_uploaded_milli = Some(100_000);
        native.downloading_status = Some(NativeUbiquitousDownloadingStatus::Current);
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NoProviderForPath,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("test fixture has no native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "native-url-resource".to_string(),
        };

        let domain = domain_for_path(&path, &hints);
        let state = storage_state_for_path(&path, domain, &hints);

        assert_eq!(domain, FileProviderDomain::ICloudDrive);
        assert_eq!(state, CloudStorageState::Downloading);
        let progress = progress_for_state(state, &hints);
        assert_eq!(progress.direction, CloudTransferDirection::Download);
        assert_eq!(progress.percent_milli, Some(12_500));
        assert!(progress.requested);
        assert!(!progress.indeterminate);
        assert_eq!(
            materialization_for_state(state),
            CloudMaterialization::InFlight
        );
        assert_eq!(
            materialization_source_for_state(state, &hints),
            CloudMaterializationSource::NativeUrlResource
        );
        assert_eq!(
            materialization_confidence_for_source(materialization_source_for_state(state, &hints)),
            CloudMaterializationConfidence::Native
        );
    }

    #[test]
    fn native_download_status_without_ubiquitous_flag_marks_remote_placeholder() {
        let path = PathBuf::from("/tmp/Document.pdf");
        let mut native = native_values();
        native.has_unresolved_conflicts = Some(false);
        native.is_downloading = Some(false);
        native.is_uploading = Some(false);
        native.download_requested = Some(false);
        native.percent_downloaded_milli = Some(0);
        native.downloading_status = Some(NativeUbiquitousDownloadingStatus::NotDownloaded);
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "native-url-resource".to_string(),
        };

        let domain = domain_for_path(&path, &hints);
        let state = storage_state_for_path(&path, domain, &hints);

        assert_eq!(domain, FileProviderDomain::ICloudDrive);
        assert_eq!(state, CloudStorageState::Evicted);
        assert_eq!(
            materialization_for_state(state),
            CloudMaterialization::RemotePlaceholder
        );
        assert_eq!(
            materialization_source_for_state(state, &hints),
            CloudMaterializationSource::NativeUrlResource
        );
        assert_eq!(progress_for_state(state, &hints).percent_milli, Some(0));
    }

    #[test]
    fn native_current_status_without_ubiquitous_flag_marks_materialized() {
        let path = PathBuf::from("/tmp/Document.pdf");
        let mut native = native_values();
        native.has_unresolved_conflicts = Some(false);
        native.is_downloading = Some(false);
        native.is_uploading = Some(false);
        native.is_uploaded = Some(true);
        native.percent_downloaded_milli = Some(100_000);
        native.downloading_status = Some(NativeUbiquitousDownloadingStatus::Current);
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "native-url-resource".to_string(),
        };

        let domain = domain_for_path(&path, &hints);
        let state = storage_state_for_path(&path, domain, &hints);

        assert_eq!(domain, FileProviderDomain::ICloudDrive);
        assert_eq!(state, CloudStorageState::Downloaded);
        assert_eq!(
            materialization_for_state(state),
            CloudMaterialization::Materialized
        );
        assert_eq!(
            materialization_source_for_state(state, &hints),
            CloudMaterializationSource::NativeUrlResource
        );
    }

    #[test]
    fn native_zero_download_percent_without_status_marks_remote_placeholder() {
        let path = PathBuf::from("/tmp/Document.pdf");
        let mut native = native_values();
        native.has_unresolved_conflicts = Some(false);
        native.is_downloading = Some(false);
        native.is_uploading = Some(false);
        native.download_requested = Some(false);
        native.percent_downloaded_milli = Some(0);
        native.downloading_status = None;
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "native-url-resource".to_string(),
        };

        let domain = domain_for_path(&path, &hints);
        let state = storage_state_for_path(&path, domain, &hints);

        assert_eq!(domain, FileProviderDomain::ICloudDrive);
        assert_eq!(state, CloudStorageState::Evicted);
        assert_eq!(
            materialization_for_state(state),
            CloudMaterialization::RemotePlaceholder
        );
        assert_eq!(
            materialization_source_for_state(state, &hints),
            CloudMaterializationSource::NativeUrlResource
        );
        assert_eq!(progress_for_state(state, &hints).percent_milli, Some(0));
    }

    #[test]
    fn native_partial_download_percent_without_status_marks_downloading() {
        let path = PathBuf::from("/tmp/Document.pdf");
        let mut native = native_values();
        native.has_unresolved_conflicts = Some(false);
        native.is_downloading = Some(false);
        native.is_uploading = Some(false);
        native.download_requested = Some(true);
        native.percent_downloaded_milli = Some(42_500);
        native.downloading_status = None;
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "native-url-resource".to_string(),
        };

        let domain = domain_for_path(&path, &hints);
        let state = storage_state_for_path(&path, domain, &hints);
        let progress = progress_for_state(state, &hints);

        assert_eq!(domain, FileProviderDomain::ICloudDrive);
        assert_eq!(state, CloudStorageState::Downloading);
        assert_eq!(
            materialization_for_state(state),
            CloudMaterialization::InFlight
        );
        assert_eq!(
            materialization_source_for_state(state, &hints),
            CloudMaterializationSource::NativeUrlResource
        );
        assert_eq!(progress.direction, CloudTransferDirection::Download);
        assert_eq!(progress.percent_milli, Some(42_500));
        assert!(progress.requested);
    }

    #[test]
    fn native_complete_download_percent_without_status_marks_materialized() {
        let path = PathBuf::from("/tmp/Document.pdf");
        let mut native = native_values();
        native.has_unresolved_conflicts = Some(false);
        native.is_downloading = Some(false);
        native.is_uploading = Some(false);
        native.percent_downloaded_milli = Some(100_000);
        native.downloading_status = None;
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "native-url-resource".to_string(),
        };

        let domain = domain_for_path(&path, &hints);
        let state = storage_state_for_path(&path, domain, &hints);

        assert_eq!(domain, FileProviderDomain::ICloudDrive);
        assert_eq!(state, CloudStorageState::Downloaded);
        assert_eq!(
            materialization_for_state(state),
            CloudMaterialization::Materialized
        );
        assert_eq!(
            materialization_source_for_state(state, &hints),
            CloudMaterializationSource::NativeUrlResource
        );
    }

    #[test]
    fn native_partial_upload_percent_without_status_marks_uploading() {
        let path = PathBuf::from("/tmp/Document.pdf");
        let mut native = native_values();
        native.has_unresolved_conflicts = Some(false);
        native.is_downloading = Some(false);
        native.is_uploading = Some(false);
        native.percent_uploaded_milli = Some(62_500);
        native.downloading_status = None;
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "native-url-resource".to_string(),
        };

        let domain = domain_for_path(&path, &hints);
        let state = storage_state_for_path(&path, domain, &hints);
        let progress = progress_for_state(state, &hints);

        assert_eq!(domain, FileProviderDomain::ICloudDrive);
        assert_eq!(state, CloudStorageState::Uploading);
        assert_eq!(
            materialization_for_state(state),
            CloudMaterialization::InFlight
        );
        assert_eq!(
            materialization_source_for_state(state, &hints),
            CloudMaterializationSource::NativeUrlResource
        );
        assert_eq!(progress.direction, CloudTransferDirection::Upload);
        assert_eq!(progress.percent_milli, Some(62_500));
    }

    #[test]
    fn native_download_requested_without_status_marks_waiting() {
        let path = PathBuf::from("/tmp/Document.pdf");
        let mut native = native_values();
        native.has_unresolved_conflicts = Some(false);
        native.is_downloading = Some(false);
        native.is_uploading = Some(false);
        native.download_requested = Some(true);
        native.downloading_status = None;
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "native-url-resource".to_string(),
        };

        let domain = domain_for_path(&path, &hints);
        let state = storage_state_for_path(&path, domain, &hints);
        let progress = progress_for_state(state, &hints);

        assert_eq!(domain, FileProviderDomain::ICloudDrive);
        assert_eq!(state, CloudStorageState::Waiting);
        assert_eq!(
            materialization_for_state(state),
            CloudMaterialization::InFlight
        );
        assert_eq!(
            materialization_source_for_state(state, &hints),
            CloudMaterializationSource::NativeUrlResource
        );
        assert_eq!(progress.direction, CloudTransferDirection::Materialize);
        assert_eq!(progress.percent_milli, None);
        assert!(progress.requested);
    }

    #[test]
    fn native_downloaded_boolean_without_identity_marks_materialized() {
        let path = PathBuf::from("/tmp/Document.pdf");
        let mut native = native_values();
        native.has_unresolved_conflicts = Some(false);
        native.is_downloaded = Some(true);
        native.is_downloading = Some(false);
        native.is_uploading = Some(false);
        native.download_requested = Some(false);
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "native-url-resource".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.domain, FileProviderDomain::ICloudDrive);
        assert_eq!(report.storage_state, CloudStorageState::Downloaded);
        assert_eq!(report.materialization, CloudMaterialization::Materialized);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::NativeUrlResource
        );
        assert_eq!(report.progress.source, "state");
        assert!(!report
            .source
            .split('+')
            .any(|source| source == "nsfileprovidermanager"));
    }

    #[test]
    fn native_downloaded_file_with_partial_upload_marks_uploading() {
        let path = PathBuf::from("/tmp/Document.pdf");
        let mut native = native_values();
        native.has_unresolved_conflicts = Some(false);
        native.is_downloaded = Some(true);
        native.is_downloading = Some(false);
        native.is_uploading = Some(false);
        native.is_uploaded = Some(false);
        native.percent_downloaded_milli = Some(100_000);
        native.percent_uploaded_milli = Some(25_000);
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "native-url-resource".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.domain, FileProviderDomain::ICloudDrive);
        assert_eq!(report.storage_state, CloudStorageState::Uploading);
        assert_eq!(report.materialization, CloudMaterialization::InFlight);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::NativeUrlResource
        );
        assert_eq!(report.progress.direction, CloudTransferDirection::Upload);
        assert_eq!(report.progress.percent_milli, Some(25_000));
        assert_eq!(
            report.progress.reason.as_deref(),
            Some("native-upload-progress")
        );
    }

    #[test]
    fn native_complete_upload_percent_without_status_marks_materialized() {
        let path = PathBuf::from("/tmp/Document.pdf");
        let mut native = native_values();
        native.has_unresolved_conflicts = Some(false);
        native.is_downloading = Some(false);
        native.is_uploading = Some(false);
        native.percent_uploaded_milli = Some(100_000);
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "native-url-resource".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.domain, FileProviderDomain::ICloudDrive);
        assert_eq!(report.storage_state, CloudStorageState::Downloaded);
        assert_eq!(report.materialization, CloudMaterialization::Materialized);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::NativeUrlResource
        );
        assert_eq!(report.progress.direction, CloudTransferDirection::Download);
        assert_eq!(report.progress.percent_milli, Some(100_000));
        assert_eq!(report.progress.reason.as_deref(), Some("materialized"));
    }

    #[test]
    fn native_uploaded_boolean_overrides_stale_partial_upload_percent() {
        let path = PathBuf::from("/tmp/Document.pdf");
        let mut native = native_values();
        native.has_unresolved_conflicts = Some(false);
        native.is_downloading = Some(false);
        native.is_uploading = Some(false);
        native.is_uploaded = Some(true);
        native.percent_uploaded_milli = Some(25_000);
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "native-url-resource".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.domain, FileProviderDomain::ICloudDrive);
        assert_eq!(report.storage_state, CloudStorageState::Downloaded);
        assert_eq!(report.materialization, CloudMaterialization::Materialized);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::NativeUrlResource
        );
        assert_eq!(report.progress.direction, CloudTransferDirection::Download);
        assert_eq!(report.progress.percent_milli, Some(100_000));
        assert_eq!(report.progress.reason.as_deref(), Some("materialized"));
    }

    #[test]
    fn native_not_downloaded_boolean_without_status_marks_remote_placeholder() {
        let path = PathBuf::from("/tmp/Document.pdf");
        let mut native = native_values();
        native.has_unresolved_conflicts = Some(false);
        native.is_downloaded = Some(false);
        native.is_downloading = Some(false);
        native.is_uploading = Some(false);
        native.download_requested = Some(false);
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "native-url-resource".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.domain, FileProviderDomain::ICloudDrive);
        assert_eq!(report.storage_state, CloudStorageState::Evicted);
        assert_eq!(
            report.materialization,
            CloudMaterialization::RemotePlaceholder
        );
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::NativeUrlResource
        );
        assert_eq!(report.progress.source, "state");
        assert_eq!(report.progress.percent_milli, Some(0));
        assert_eq!(report.commands.download, CloudCommandState::Enabled);
        assert_eq!(report.commands.reason, None);
    }

    #[test]
    fn native_not_downloaded_boolean_overrides_downloaded_fixture_name() {
        let path = PathBuf::from("/tmp/Downloaded.icloud.md");
        let mut native = native_values();
        native.has_unresolved_conflicts = Some(false);
        native.is_downloaded = Some(false);
        native.is_downloading = Some(false);
        native.is_uploading = Some(false);
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "native-url-resource".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.storage_state, CloudStorageState::Evicted);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::NativeUrlResource
        );
    }

    #[test]
    fn path_only_icloud_name_is_local_without_native_evidence() {
        let path = PathBuf::from("/tmp/Downloaded.icloud.md");
        let hints = CloudHints {
            native: native_values(),
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "fixture-name".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.domain, FileProviderDomain::Local);
        assert_eq!(report.storage_state, CloudStorageState::LocalOnly);
        assert_eq!(
            report.materialization,
            CloudMaterialization::NotProviderBacked
        );
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::Filesystem
        );
    }

    #[test]
    fn native_ubiquitous_unknown_materialization_ignores_xattr_fallback_state() {
        let path = PathBuf::from("/tmp/Downloaded.icloud.md");
        let mut native = native_values();
        native.is_ubiquitous = Some(true);
        native.has_unresolved_conflicts = Some(false);
        native.is_downloading = Some(false);
        native.is_uploading = Some(false);
        let hints = CloudHints {
            native,
            native_identity: identity_not_queried(),
            xattrs: vec!["com.apple.fileprovider.state".to_string()],
            xattr_values: vec!["isDownloaded=true; materialized=true".to_string()],
            provider_identifier: None,
            source: "native-url-resource+xattr".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.domain, FileProviderDomain::ICloudDrive);
        assert_eq!(report.storage_state, CloudStorageState::Unknown);
        assert_eq!(report.materialization, CloudMaterialization::Unknown);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::NativeUrlResource
        );
        assert_eq!(
            report.materialization_reason.as_deref(),
            Some("native-url-resource-unknown")
        );
    }

    #[test]
    fn native_ubiquitous_unknown_materialization_ignores_filename_state_words() {
        let path = PathBuf::from("/tmp/ConflictDownloading.icloud.md");
        let mut native = native_values();
        native.is_ubiquitous = Some(true);
        native.has_unresolved_conflicts = Some(false);
        native.is_downloading = Some(false);
        native.is_uploading = Some(false);
        let hints = CloudHints {
            native,
            native_identity: identity_not_queried(),
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "fixture-name+native-url-resource".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.domain, FileProviderDomain::ICloudDrive);
        assert_eq!(report.storage_state, CloudStorageState::Unknown);
        assert_eq!(report.materialization, CloudMaterialization::Unknown);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::NativeUrlResource
        );
    }

    #[test]
    fn native_partial_download_percent_without_boolean_marks_downloading() {
        let path = PathBuf::from("/tmp/Remote.dat");
        let mut native = native_values();
        native.is_ubiquitous = Some(true);
        native.has_unresolved_conflicts = Some(false);
        native.is_downloading = Some(false);
        native.is_uploading = Some(false);
        native.percent_downloaded_milli = Some(37_500);
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "native-url-resource".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.storage_state, CloudStorageState::Downloading);
        assert_eq!(report.materialization, CloudMaterialization::InFlight);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::NativeUrlResource
        );
        assert_eq!(report.progress.percent_milli, Some(37_500));
        assert_eq!(
            report.progress.reason.as_deref(),
            Some("native-download-progress")
        );
    }

    #[test]
    fn native_zero_download_percent_without_boolean_marks_remote_placeholder() {
        let path = PathBuf::from("/tmp/Remote.dat");
        let mut native = native_values();
        native.is_ubiquitous = Some(true);
        native.has_unresolved_conflicts = Some(false);
        native.is_downloading = Some(false);
        native.is_uploading = Some(false);
        native.percent_downloaded_milli = Some(0);
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "native-url-resource".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.storage_state, CloudStorageState::Evicted);
        assert_eq!(
            report.materialization,
            CloudMaterialization::RemotePlaceholder
        );
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::NativeUrlResource
        );
        assert_eq!(report.progress.percent_milli, Some(0));
    }

    #[test]
    fn native_partial_upload_percent_without_boolean_marks_uploading() {
        let path = PathBuf::from("/tmp/Local.dat");
        let mut native = native_values();
        native.is_ubiquitous = Some(true);
        native.has_unresolved_conflicts = Some(false);
        native.is_downloading = Some(false);
        native.is_uploading = Some(false);
        native.percent_uploaded_milli = Some(25_000);
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "native-url-resource".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.storage_state, CloudStorageState::Uploading);
        assert_eq!(report.materialization, CloudMaterialization::InFlight);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::NativeUrlResource
        );
        assert_eq!(report.progress.percent_milli, Some(25_000));
        assert_eq!(
            report.progress.reason.as_deref(),
            Some("native-upload-progress")
        );
    }

    #[test]
    fn native_conflict_state_overrides_local_filename_fallbacks() {
        let path = PathBuf::from("/tmp/Report.md");
        let mut native = native_values();
        native.is_ubiquitous = Some(true);
        native.has_unresolved_conflicts = Some(true);
        native.is_downloading = Some(false);
        native.is_uploading = Some(false);
        native.is_uploaded = Some(true);
        native.download_requested = Some(false);
        native.percent_downloaded_milli = Some(100_000);
        native.percent_uploaded_milli = Some(100_000);
        native.downloading_status = Some(NativeUbiquitousDownloadingStatus::Downloaded);
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NoProviderForPath,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("test fixture has no native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "native-url-resource".to_string(),
        };

        let domain = domain_for_path(&path, &hints);
        let state = storage_state_for_path(&path, domain, &hints);

        assert_eq!(domain, FileProviderDomain::ICloudDrive);
        assert_eq!(state, CloudStorageState::Conflict);
        assert_eq!(
            materialization_for_state(state),
            CloudMaterialization::Conflict
        );
    }

    #[test]
    fn native_ubiquitous_error_reports_offline_materialization() {
        let path = PathBuf::from("/tmp/Report.md");
        let mut native = native_values();
        native.is_ubiquitous = Some(true);
        native.has_unresolved_conflicts = Some(false);
        native.is_downloading = Some(false);
        native.is_uploading = Some(false);
        native.is_uploaded = Some(false);
        native.downloading_error = Some(NativeUbiquitousError {
            code: Some(4_355),
            description: Some("The iCloud server is unavailable.".to_string()),
        });
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NoProviderForPath,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("test fixture has no native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "native-url-resource".to_string(),
        };

        let domain = domain_for_path(&path, &hints);
        let state = storage_state_for_path(&path, domain, &hints);

        assert_eq!(domain, FileProviderDomain::ICloudDrive);
        assert_eq!(state, CloudStorageState::Offline);
        assert_eq!(
            materialization_for_state(state),
            CloudMaterialization::Offline
        );
        assert_eq!(
            materialization_source_for_state(state, &hints),
            CloudMaterializationSource::NativeUrlResource
        );
    }

    #[test]
    fn native_url_unsupported_unknown_state_reports_typed_source() {
        let path = PathBuf::from("/tmp/Remote.fileprovider");
        let mut native = native_values();
        native.status = gfm_mac_sys::NativeFileProviderStatus::UnsupportedPath;
        native.reason = Some("native URL resource values unsupported".to_string());
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "fixture-name".to_string(),
        };

        let domain = domain_for_path(&path, &hints);
        let state = storage_state_for_path(&path, domain, &hints);

        assert_eq!(domain, FileProviderDomain::FileProvider);
        assert_eq!(state, CloudStorageState::Unknown);
        assert_eq!(
            materialization_for_state(state),
            CloudMaterialization::Unknown
        );
        assert_eq!(
            materialization_source_for_state(state, &hints),
            CloudMaterializationSource::NativeUrlResourceUnsupported
        );
        assert_eq!(
            materialization_reason_for_state(state, &hints).as_deref(),
            Some("native URL resource values unsupported")
        );

        let report = FileProviderStateReport::from_hints(path, hints);
        assert!(report.as_tsv().contains(
            "\tmaterialization-source=native-url-resource:unsupported\tmaterialization-confidence=native\tmaterialization-reason=native URL resource values unsupported\t"
        ));
    }

    #[test]
    fn native_url_unavailable_unknown_state_reports_typed_source() {
        let path = PathBuf::from("/tmp/Remote.fileprovider");
        let mut native = native_values();
        native.status = gfm_mac_sys::NativeFileProviderStatus::Unavailable;
        native.reason = Some("native FileProvider URL resource values unavailable".to_string());
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "fixture-name".to_string(),
        };

        let domain = domain_for_path(&path, &hints);
        let state = storage_state_for_path(&path, domain, &hints);

        assert_eq!(domain, FileProviderDomain::FileProvider);
        assert_eq!(state, CloudStorageState::Unknown);
        assert_eq!(
            materialization_source_for_state(state, &hints),
            CloudMaterializationSource::NativeUrlResourceUnavailable
        );
        assert_eq!(
            materialization_reason_for_state(state, &hints).as_deref(),
            Some("native FileProvider URL resource values unavailable")
        );

        let report = FileProviderStateReport::from_hints(path, hints);
        assert!(report.as_tsv().contains(
            "\tmaterialization-source=native-url-resource:unavailable\tmaterialization-confidence=native\tmaterialization-reason=native FileProvider URL resource values unavailable\t"
        ));
    }

    #[test]
    fn native_unavailable_state_ignores_bare_filename_state_words() {
        let path = PathBuf::from("/tmp/OfflineConflictDownloading.fileprovider");
        let mut native = native_values();
        native.status = gfm_mac_sys::NativeFileProviderStatus::Unavailable;
        native.reason = Some("native FileProvider URL resource values unavailable".to_string());
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "fixture-name".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.domain, FileProviderDomain::FileProvider);
        assert_eq!(report.storage_state, CloudStorageState::Unknown);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::NativeUrlResourceUnavailable
        );
        assert_eq!(
            report.materialization_reason.as_deref(),
            Some("native FileProvider URL resource values unavailable")
        );
    }

    #[test]
    fn native_identity_allows_filename_state_words_as_explicit_provider_hints() {
        let path = PathBuf::from("/tmp/Conflict.fileprovider");
        let hints = CloudHints {
            native: native_values(),
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::Available,
                item_identifier: Some("item".to_string()),
                domain_identifier: Some("com.example.drive".to_string()),
                reason: None,
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: Some("com.example.drive".to_string()),
            source: "nsfileprovidermanager+fixture-name".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.domain, FileProviderDomain::FileProvider);
        assert_eq!(report.storage_state, CloudStorageState::Conflict);
        assert_eq!(report.materialization, CloudMaterialization::Conflict);
    }

    #[test]
    fn native_url_missing_unknown_state_reports_typed_source() {
        let path = PathBuf::from("/tmp/Remote.fileprovider");
        let mut native = native_values();
        native.status = gfm_mac_sys::NativeFileProviderStatus::Missing;
        native.reason = Some("native FileProvider URL resource path missing".to_string());
        let hints = CloudHints {
            native,
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "fixture-name".to_string(),
        };

        let domain = domain_for_path(&path, &hints);
        let state = storage_state_for_path(&path, domain, &hints);

        assert_eq!(domain, FileProviderDomain::FileProvider);
        assert_eq!(state, CloudStorageState::Unknown);
        assert_eq!(
            materialization_source_for_state(state, &hints),
            CloudMaterializationSource::NativeUrlResourceMissing
        );
        assert_eq!(
            materialization_reason_for_state(state, &hints).as_deref(),
            Some("native FileProvider URL resource path missing")
        );

        let report = FileProviderStateReport::from_hints(path, hints);
        assert!(report.as_tsv().contains(
            "\tmaterialization-source=native-url-resource:missing\tmaterialization-confidence=native\tmaterialization-reason=native FileProvider URL resource path missing\t"
        ));
    }

    #[test]
    fn native_identity_unavailable_unknown_state_reports_typed_source() {
        let path = PathBuf::from("/tmp/Remote.fileprovider");
        let hints = CloudHints {
            native: native_values(),
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::Unavailable,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("FileProvider identity path probe unavailable".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "fixture-name".to_string(),
        };

        let domain = domain_for_path(&path, &hints);
        let state = storage_state_for_path(&path, domain, &hints);

        assert_eq!(domain, FileProviderDomain::FileProvider);
        assert_eq!(state, CloudStorageState::Unknown);
        assert_eq!(
            materialization_source_for_state(state, &hints),
            CloudMaterializationSource::NativeFileProviderIdentityUnavailable
        );
        assert_eq!(
            materialization_reason_for_state(state, &hints).as_deref(),
            Some("FileProvider identity path probe unavailable")
        );

        let report = FileProviderStateReport::from_hints(path, hints);
        assert!(report.as_tsv().contains(
            "\tmaterialization-source=nsfileprovidermanager:unavailable\tmaterialization-confidence=provider-identity\tmaterialization-reason=FileProvider identity path probe unavailable\t"
        ));
    }

    #[test]
    fn native_identity_missing_unknown_state_reports_typed_source() {
        let path = PathBuf::from("/tmp/Remote.fileprovider");
        let hints = CloudHints {
            native: native_values(),
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::Missing,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("FileProvider identity path missing".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "fixture-name".to_string(),
        };

        let domain = domain_for_path(&path, &hints);
        let state = storage_state_for_path(&path, domain, &hints);

        assert_eq!(domain, FileProviderDomain::FileProvider);
        assert_eq!(state, CloudStorageState::Unknown);
        assert_eq!(
            materialization_source_for_state(state, &hints),
            CloudMaterializationSource::NativeFileProviderIdentityMissing
        );
        assert_eq!(
            materialization_reason_for_state(state, &hints).as_deref(),
            Some("FileProvider identity path missing")
        );

        let report = FileProviderStateReport::from_hints(path, hints);
        assert!(report.as_tsv().contains(
            "\tmaterialization-source=nsfileprovidermanager:missing\tmaterialization-confidence=provider-identity\tmaterialization-reason=FileProvider identity path missing\t"
        ));
    }

    #[test]
    fn native_identity_no_provider_reports_typed_local_only_source() {
        let path = PathBuf::from("/tmp/Local.md");
        let hints = CloudHints {
            native: native_values(),
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NoProviderForPath,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("NSFileProviderManager returned no provider for path".to_string()),
            },
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "nsfileprovidermanager".to_string(),
        };

        let domain = domain_for_path(&path, &hints);
        let state = storage_state_for_path(&path, domain, &hints);

        assert_eq!(domain, FileProviderDomain::Local);
        assert_eq!(state, CloudStorageState::LocalOnly);
        assert_eq!(
            materialization_source_for_state(state, &hints),
            CloudMaterializationSource::NativeFileProviderIdentityNoProviderForPath
        );
        assert_eq!(
            materialization_confidence_for_source(materialization_source_for_state(state, &hints)),
            CloudMaterializationConfidence::ProviderIdentity
        );
        assert_eq!(
            materialization_reason_for_state(state, &hints).as_deref(),
            Some("NSFileProviderManager returned no provider for path")
        );

        let report = FileProviderStateReport::from_hints(path, hints);
        assert!(report.as_tsv().contains(
            "\tmaterialization-source=nsfileprovidermanager:no-provider-for-path\tmaterialization-confidence=provider-identity\tmaterialization-reason=NSFileProviderManager returned no provider for path\t"
        ));
    }

    #[test]
    fn native_identity_uncertainty_sources_preserve_status() {
        for (status, source, source_label, reason) in [
            (
                NativeFileProviderIdentityStatus::ProviderUnavailable,
                CloudMaterializationSource::NativeFileProviderIdentityProviderUnavailable,
                "nsfileprovidermanager:provider-unavailable",
                "NSFileProviderManager class is unavailable",
            ),
            (
                NativeFileProviderIdentityStatus::TimedOut,
                CloudMaterializationSource::NativeFileProviderIdentityTimedOut,
                "nsfileprovidermanager:timed-out",
                "FileProvider identity request timed out",
            ),
            (
                NativeFileProviderIdentityStatus::Failed,
                CloudMaterializationSource::NativeFileProviderIdentityFailed,
                "nsfileprovidermanager:failed",
                "FileProvider identity request failed",
            ),
        ] {
            let path = PathBuf::from("/tmp/Remote.fileprovider");
            let hints = CloudHints {
                native: native_values(),
                native_identity: NativeFileProviderIdentity {
                    status,
                    item_identifier: None,
                    domain_identifier: None,
                    reason: Some(reason.to_string()),
                },
                xattrs: Vec::new(),
                xattr_values: Vec::new(),
                provider_identifier: None,
                source: "fixture-name".to_string(),
            };

            let domain = domain_for_path(&path, &hints);
            let state = storage_state_for_path(&path, domain, &hints);

            assert_eq!(domain, FileProviderDomain::FileProvider);
            assert_eq!(state, CloudStorageState::Unknown);
            assert_eq!(materialization_source_for_state(state, &hints), source);
            assert_eq!(
                materialization_reason_for_state(state, &hints).as_deref(),
                Some(reason)
            );

            let report = FileProviderStateReport::from_hints(path, hints);
            assert!(report.as_tsv().contains(&format!(
                "\tmaterialization-source={source_label}\tmaterialization-confidence=provider-identity\tmaterialization-reason={reason}\t"
            )));
        }
    }

    #[test]
    fn materialization_report_marks_xattr_fallback_placeholders() {
        let path = PathBuf::from("/tmp/Remote.icloud-placeholder");
        let hints = CloudHints {
            native: native_values(),
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("test fixture has no native manager identity".to_string()),
            },
            xattrs: vec!["com.apple.icloud.placeholder".to_string()],
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "fixture-name+xattr".to_string(),
        };

        let domain = domain_for_path(&path, &hints);
        let state = storage_state_for_path(&path, domain, &hints);

        assert_eq!(domain, FileProviderDomain::ICloudDrive);
        assert_eq!(state, CloudStorageState::Evicted);
        assert_eq!(
            materialization_for_state(state),
            CloudMaterialization::RemotePlaceholder
        );
        assert_eq!(
            materialization_source_for_state(state, &hints),
            CloudMaterializationSource::XattrFallback
        );
    }

    #[test]
    fn xattr_value_conflict_overrides_downloaded_fixture_name() {
        let path = PathBuf::from("/tmp/Downloaded.icloud.md");
        let hints = CloudHints {
            native: native_values(),
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: vec!["com.apple.fileprovider.state".to_string()],
            xattr_values: vec!["unresolved-conflict".to_string()],
            provider_identifier: None,
            source: "fixture-name+xattr".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.domain, FileProviderDomain::ICloudDrive);
        assert_eq!(report.storage_state, CloudStorageState::Conflict);
        assert_eq!(report.materialization, CloudMaterialization::Conflict);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::XattrFallback
        );
    }

    #[test]
    fn xattr_value_not_downloaded_marks_remote_placeholder() {
        let path = PathBuf::from("/tmp/Remote.icloud.md");
        let hints = CloudHints {
            native: native_values(),
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: vec!["com.apple.fileprovider.state".to_string()],
            xattr_values: vec!["not-downloaded".to_string()],
            provider_identifier: None,
            source: "fixture-name+xattr".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.storage_state, CloudStorageState::Evicted);
        assert_eq!(
            report.materialization,
            CloudMaterialization::RemotePlaceholder
        );
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::XattrFallback
        );
        assert_eq!(report.commands.download, CloudCommandState::Disabled);
        assert_eq!(
            report.commands.reason.as_deref(),
            Some("not-native-provider-backed")
        );
    }

    #[test]
    fn xattr_value_downloaded_false_marks_remote_placeholder() {
        let path = PathBuf::from("/tmp/Downloaded.icloud.md");
        let hints = CloudHints {
            native: native_values(),
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: vec!["com.apple.fileprovider.state".to_string()],
            xattr_values: vec!["isDownloaded=false; isDownloading=false".to_string()],
            provider_identifier: None,
            source: "fixture-name+xattr".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.domain, FileProviderDomain::ICloudDrive);
        assert_eq!(report.storage_state, CloudStorageState::Evicted);
        assert_eq!(
            report.materialization,
            CloudMaterialization::RemotePlaceholder
        );
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::XattrFallback
        );
        assert_eq!(report.progress.percent_milli, Some(0));
        assert_eq!(report.commands.download, CloudCommandState::Disabled);
    }

    #[test]
    fn xattr_value_conflict_false_does_not_override_evicted_state() {
        let path = PathBuf::from("/tmp/Remote.icloud.md");
        let hints = CloudHints {
            native: native_values(),
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: vec!["com.apple.fileprovider.state".to_string()],
            xattr_values: vec!["hasUnresolvedConflicts=false; isDownloaded=false".to_string()],
            provider_identifier: None,
            source: "fixture-name+xattr".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.storage_state, CloudStorageState::Evicted);
        assert!(!report.conflict);
        assert_eq!(
            report.materialization,
            CloudMaterialization::RemotePlaceholder
        );
    }

    #[test]
    fn xattr_value_conflict_false_alone_keeps_materialized_state() {
        let root = unique_temp_dir();
        let path = root.join("Remote.icloud.md");
        fs::write(&path, "remote").unwrap();
        let hints = CloudHints {
            native: native_values(),
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: vec!["com.apple.fileprovider.state".to_string()],
            xattr_values: vec!["hasUnresolvedConflicts=false".to_string()],
            provider_identifier: None,
            source: "fixture-name+xattr".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.storage_state, CloudStorageState::Downloaded);
        assert!(!report.conflict);
        assert_eq!(report.materialization, CloudMaterialization::Materialized);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::XattrFallback
        );

        let suffix_path = root.join("Suffix.icloud.md");
        fs::write(&suffix_path, "suffix").unwrap();
        let suffix_hints = CloudHints {
            native: native_values(),
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: vec!["com.apple.fileprovider.state".to_string()],
            xattr_values: vec!["isDownloaded=falsepositive".to_string()],
            provider_identifier: None,
            source: "fixture-name+xattr".to_string(),
        };

        let suffix_report = FileProviderStateReport::from_hints(suffix_path, suffix_hints);

        assert_eq!(suffix_report.storage_state, CloudStorageState::Downloaded);
        assert_eq!(
            suffix_report.materialization,
            CloudMaterialization::Materialized
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn xattr_value_current_overrides_placeholder_fixture_name_as_materialized() {
        let path = PathBuf::from("/tmp/Remote.icloud-placeholder");
        let hints = CloudHints {
            native: native_values(),
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: vec!["com.apple.fileprovider.state".to_string()],
            xattr_values: vec!["current".to_string()],
            provider_identifier: None,
            source: "fixture-name+xattr".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.domain, FileProviderDomain::ICloudDrive);
        assert_eq!(report.storage_state, CloudStorageState::Downloaded);
        assert_eq!(report.materialization, CloudMaterialization::Materialized);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::XattrFallback
        );
        assert_eq!(report.progress.direction, CloudTransferDirection::Download);
        assert!(report.progress.complete);
        assert_eq!(report.badges, vec![CloudBadge::AvailableOffline]);
        assert_eq!(report.commands.download, CloudCommandState::Disabled);
        assert_eq!(report.commands.evict, CloudCommandState::Disabled);
        assert_eq!(
            report.commands.reason.as_deref(),
            Some("not-native-provider-backed")
        );
    }

    #[test]
    fn xattr_value_downloading_overrides_downloaded_fixture_name_as_in_flight() {
        let path = PathBuf::from("/tmp/Downloaded.icloud.md");
        let hints = CloudHints {
            native: native_values(),
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("hot path skipped native manager identity".to_string()),
            },
            xattrs: vec!["com.apple.fileprovider.state".to_string()],
            xattr_values: vec!["download-in-progress".to_string()],
            provider_identifier: None,
            source: "fixture-name+xattr".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.domain, FileProviderDomain::ICloudDrive);
        assert_eq!(report.storage_state, CloudStorageState::Downloading);
        assert_eq!(report.materialization, CloudMaterialization::InFlight);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::XattrFallback
        );
        assert_eq!(report.progress.direction, CloudTransferDirection::Download);
        assert!(report.progress.requested);
        assert_eq!(
            report.commands.reason.as_deref(),
            Some("provider-operation-in-flight")
        );
    }

    #[test]
    fn state_read_uses_bounded_provider_xattr_value_signal() {
        let root = unique_temp_dir();
        let path = root.join("Remote.icloud.md");
        fs::write(&path, "remote").unwrap();
        xattr::set(&path, "com.apple.fileprovider.state", b"not-downloaded").unwrap();
        xattr::set(&path, "com.apple.fileprovider.domain", b"com.example.drive").unwrap();
        let oversized_value = vec![b'a'; MAX_PROVIDER_XATTR_VALUE_BYTES + 1];
        xattr::set(&path, "com.apple.fileprovider.large", &oversized_value).unwrap();

        let report = FileProviderStateReport::read_path(&path).unwrap();

        assert_eq!(report.storage_state, CloudStorageState::Evicted);
        assert_eq!(
            report.materialization,
            CloudMaterialization::RemotePlaceholder
        );
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::XattrFallback
        );
        assert!(report.source.split('+').any(|source| source == "xattr"));
        assert_eq!(
            report.provider_identifier.as_deref(),
            Some("com.example.drive")
        );
        assert!(!report
            .as_tsv()
            .contains(&"a".repeat(MAX_PROVIDER_XATTR_VALUE_BYTES + 1)));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn state_read_treats_unknown_provider_xattr_as_unknown_materialization() {
        let root = unique_temp_dir();
        let path = root.join("Unknown.icloud.md");
        fs::write(&path, "unknown").unwrap();
        xattr::set(&path, "com.apple.fileprovider.state", b"unknown=true").unwrap();
        xattr::set(&path, "com.apple.fileprovider.domain", b"com.example.drive").unwrap();

        let report = FileProviderStateReport::read_path(&path).unwrap();

        assert_eq!(report.domain, FileProviderDomain::ICloudDrive);
        assert_eq!(report.storage_state, CloudStorageState::Unknown);
        assert_eq!(report.materialization, CloudMaterialization::Unknown);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::XattrFallback
        );
        assert_eq!(
            report.materialization_reason.as_deref(),
            Some("unknown-provider-state")
        );
        assert_eq!(
            report.progress.reason.as_deref(),
            Some("unknown-provider-state")
        );
        assert_eq!(
            report.commands.reason.as_deref(),
            Some("unknown-provider-state")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hot_state_read_ignores_provider_xattrs_for_plain_local_file_without_provider_path_hints() {
        let root = unique_temp_dir();
        let path = root.join("Local.md");
        fs::write(&path, "local").unwrap();
        xattr::set(&path, "com.apple.fileprovider.state", b"not-downloaded").unwrap();
        xattr::set(&path, "com.apple.fileprovider.domain", b"com.example.drive").unwrap();

        let report = FileProviderStateReport::read_path(&path).unwrap();

        assert_eq!(report.domain, FileProviderDomain::Local);
        assert_eq!(report.storage_state, CloudStorageState::LocalOnly);
        assert_eq!(
            report.materialization,
            CloudMaterialization::NotProviderBacked
        );
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::Filesystem
        );
        assert_eq!(
            report.materialization_reason.as_deref(),
            Some("not-fileprovider-backed")
        );
        assert!(report.provider_identifier.is_none());
        assert!(!report.source.split('+').any(|source| source == "xattr"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn provider_xattr_gate_skips_plain_local_native_state() {
        let mut native = native_values();
        native.is_ubiquitous = Some(false);

        assert!(!should_read_provider_xattrs(
            &native,
            &identity_not_queried(),
            false
        ));
        assert!(!should_read_provider_xattrs(
            &native,
            &identity_not_queried(),
            true
        ));
    }

    #[test]
    fn native_local_only_resource_suppresses_provider_path_hints() {
        let path = PathBuf::from("/tmp/Downloaded.icloud.md");
        let mut native = native_values();
        native.is_ubiquitous = Some(false);
        assert!(!should_include_provider_path_sources(
            &native,
            &identity_not_queried()
        ));
        let hints = CloudHints {
            native,
            native_identity: identity_not_queried(),
            xattrs: Vec::new(),
            xattr_values: Vec::new(),
            provider_identifier: None,
            source: "native-url-resource".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.domain, FileProviderDomain::Local);
        assert_eq!(report.storage_state, CloudStorageState::LocalOnly);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::NativeUrlResource
        );
        assert_eq!(
            report.materialization_reason.as_deref(),
            Some("native-url-resource-not-provider-backed")
        );
        assert_eq!(report.source, "native-url-resource");
        assert!(!report
            .source
            .split('+')
            .any(|source| source == "fixture-name"));
    }

    #[test]
    fn provider_xattr_gate_keeps_fallback_paths_for_provider_uncertainty() {
        let mut missing_native = native_values();
        missing_native.status = NativeFileProviderStatus::Missing;
        let mut provider_native = native_values();
        provider_native.is_ubiquitous = Some(true);
        let identity = NativeFileProviderIdentity {
            status: NativeFileProviderIdentityStatus::Available,
            item_identifier: Some("item".to_string()),
            domain_identifier: Some("com.example.drive".to_string()),
            reason: None,
        };

        assert!(should_include_provider_path_sources(
            &missing_native,
            &identity_not_queried()
        ));
        assert!(should_read_provider_xattrs(
            &native_values(),
            &identity_not_queried(),
            true
        ));
        assert!(should_read_provider_xattrs(
            &missing_native,
            &identity_not_queried(),
            false
        ));
        assert!(should_read_provider_xattrs(
            &provider_native,
            &identity_not_queried(),
            false
        ));
        assert!(should_read_provider_xattrs(
            &native_values(),
            &identity,
            false
        ));
    }

    fn mark_evicted_fixture(path: &Path) {
        xattr::set(path, "com.apple.icloud.placeholder", b"1").unwrap();
    }

    fn unique_temp_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "gfm-fileprovider-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::SeqCst),
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn native_values() -> NativeFileProviderResourceValues {
        NativeFileProviderResourceValues {
            is_ubiquitous: None,
            has_unresolved_conflicts: None,
            is_downloaded: None,
            is_downloading: None,
            is_uploading: None,
            is_uploaded: None,
            download_requested: None,
            percent_downloaded_milli: None,
            percent_uploaded_milli: None,
            downloading_status: None,
            downloading_error: None,
            uploading_error: None,
            is_excluded_from_sync: None,
            status: NativeFileProviderStatus::Available,
            reason: None,
        }
    }
}
