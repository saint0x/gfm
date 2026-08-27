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
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

const ICLOUD_DRIVE_COMPONENT: &str = "com~apple~CloudDocs";

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
    NativeUrlResourceUnavailable,
    NativeUrlResourceUnsupported,
    NativeFileProviderIdentityUnknown,
    NativeFileProviderIdentityUnavailable,
    NativeFileProviderIdentityUnsupported,
    XattrFallback,
    PathFallback,
    Filesystem,
    StateFallback,
}

impl CloudMaterializationSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NativeUrlResource => "native-url-resource",
            Self::NativeUrlResourceUnavailable => "native-url-resource:unavailable",
            Self::NativeUrlResourceUnsupported => "native-url-resource:unsupported",
            Self::NativeFileProviderIdentityUnknown => "nsfileprovidermanager:unknown",
            Self::NativeFileProviderIdentityUnavailable => "nsfileprovidermanager:unavailable",
            Self::NativeFileProviderIdentityUnsupported => "nsfileprovidermanager:unsupported",
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
        let path = path.as_ref().to_path_buf();
        if !path.exists() && !is_evicted_placeholder_path(&path) {
            return Err(GfmError::io(&path, "path does not exist"));
        }
        let hints = CloudHints::read_with_identity(&path);
        Ok(Self::from_hints_and_domains(
            path,
            hints,
            enumerate_fileprovider_domains(),
        ))
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
        let path = path.as_ref().to_path_buf();
        let state = FileProviderStateReport::read_path(&path)?;
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
        let path = path.as_ref().to_path_buf();
        let state = FileProviderStateReport::read_path(&path)?;
        let has_unresolved_conflict = state.storage_state == CloudStorageState::Conflict;
        let affected_paths = if has_unresolved_conflict {
            vec![path.clone()]
        } else {
            Vec::new()
        };
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
    Failed,
}

impl FileProviderOperationDisposition {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Refused => "refused",
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileProviderObservedInvalidation {
    pub events: usize,
    pub paths: Vec<PathBuf>,
    pub report: FileProviderStateInvalidationReport,
}

pub struct FileProviderStateObserver {
    stream: FileEventStream,
    snapshot: FileProviderStateSnapshot,
}

impl FileProviderInvalidationReport {
    pub fn evaluate(
        path: impl AsRef<Path>,
        previous: CloudStorageState,
    ) -> Result<FileProviderInvalidationReport> {
        let path = path.as_ref().to_path_buf();
        let current = FileProviderStateReport::read_path(&path)?;
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
        for path in paths {
            let state = FileProviderStateReport::read_path(&path)?.storage_state;
            entries.push(FileProviderStateSnapshotEntry { path, state });
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        entries.dedup_by(|left, right| left.path == right.path);
        Ok(Self { entries })
    }

    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).map_err(|err| GfmError::io(path, err))?;
        let mut lines = text.lines();
        match lines.next() {
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
        for (line_index, line) in lines.enumerate() {
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
            entries.push(FileProviderStateSnapshotEntry {
                state: CloudStorageState::parse(fields[0])?,
                path: PathBuf::from(unescape_field(fields[1])),
            });
        }
        Ok(Self { entries })
    }

    pub fn write(&self, path: impl AsRef<Path>) -> Result<()> {
        let mut snapshot = self.clone();
        snapshot
            .entries
            .sort_by(|left, right| left.path.cmp(&right.path));
        snapshot
            .entries
            .dedup_by(|left, right| left.path == right.path);
        let mut output = String::from("gfm-fileprovider-state-v1\n");
        for entry in &snapshot.entries {
            output.push_str(&format!(
                "{}\t{}\n",
                entry.state.as_str(),
                escape_field(&entry.path.to_string_lossy())
            ));
        }
        atomic_write_text(path.as_ref(), &output)
    }

    fn previous_state_for(&self, path: &Path) -> Option<CloudStorageState> {
        self.entries
            .iter()
            .find(|entry| entry.path == path)
            .map(|entry| entry.state)
    }

    fn contains_path(&self, path: &Path) -> bool {
        self.previous_state_for(path).is_some()
    }
}

impl FileProviderStateInvalidationReport {
    pub fn evaluate(
        previous: Option<&FileProviderStateSnapshot>,
        current_paths: impl IntoIterator<Item = PathBuf>,
    ) -> Result<(Self, FileProviderStateSnapshot)> {
        let initialized = previous.is_none();
        let mut changes = Vec::new();
        let mut current_entries = Vec::new();
        for path in current_paths {
            let previous_state = previous.and_then(|snapshot| snapshot.previous_state_for(&path));
            let current = if path.exists() || is_evicted_placeholder_path(&path) {
                Some(FileProviderStateReport::read_path(&path)?)
            } else if previous_state.is_some() {
                Some(FileProviderStateReport::removed(path.clone()))
            } else {
                None
            };
            let current = current.ok_or_else(|| GfmError::io(&path, "path does not exist"))?;
            let previous_state = previous
                .and_then(|snapshot| snapshot.previous_state_for(&path))
                .unwrap_or(CloudStorageState::LocalOnly);
            let change =
                FileProviderInvalidationReport::from_current(path.clone(), previous_state, current);
            if change.current.source != "removed" {
                current_entries.push(FileProviderStateSnapshotEntry {
                    path,
                    state: change.current.storage_state,
                });
            }
            if initialized || change.state_changed {
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

impl FileProviderObservedInvalidation {
    pub fn evaluate(
        previous: Option<&FileProviderStateSnapshot>,
        events: impl IntoIterator<Item = FileEvent>,
    ) -> Result<(Self, FileProviderStateSnapshot)> {
        let mut event_count = 0;
        let mut paths = BTreeSet::new();
        for event in events {
            event_count += 1;
            for path in paths_for_fileprovider_event(previous, &event) {
                paths.insert(path);
            }
        }
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
            let (report, event_snapshot) =
                FileProviderStateInvalidationReport::evaluate(previous, paths.clone())?;
            let snapshot = merge_observed_snapshot(previous, &paths, event_snapshot);
            (report, snapshot)
        };
        Ok((
            Self {
                events: event_count,
                paths,
                report,
            },
            snapshot,
        ))
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = vec![format!(
            "fileprovider-observed-invalidation\tevents={}\tpaths={}",
            self.events,
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
) -> FileProviderStateSnapshot {
    let mut entries = previous
        .map(|snapshot| snapshot.entries.clone())
        .unwrap_or_default();
    entries.retain(|entry| !observed_paths.iter().any(|path| path == &entry.path));
    entries.extend(event_snapshot.entries);
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    entries.dedup_by(|left, right| left.path == right.path);
    FileProviderStateSnapshot { entries }
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
        let mut events = Vec::new();
        for _ in 0..max_events {
            match self.stream.try_recv() {
                Some(Ok(event)) => events.push(event),
                Some(Err(err)) => return Err(err),
                None => break,
            }
        }
        if events.is_empty() {
            return Ok(None);
        }
        self.apply_events(events).map(Some)
    }

    fn apply_events(
        &mut self,
        events: impl IntoIterator<Item = FileEvent>,
    ) -> Result<FileProviderObservedInvalidation> {
        let (observed, snapshot) =
            FileProviderObservedInvalidation::evaluate(Some(&self.snapshot), events)?;
        self.snapshot = snapshot;
        Ok(observed)
    }
}

fn paths_for_fileprovider_event(
    previous: Option<&FileProviderStateSnapshot>,
    event: &FileEvent,
) -> Vec<PathBuf> {
    match &event.kind {
        FileEventKind::Rename { from, to } => [from, to]
            .into_iter()
            .filter(|path| is_observable_fileprovider_path(previous, path))
            .cloned()
            .collect(),
        FileEventKind::Remove => {
            if is_observable_fileprovider_path(previous, &event.path) {
                vec![event.path.clone()]
            } else {
                Vec::new()
            }
        }
        FileEventKind::Create
        | FileEventKind::Metadata
        | FileEventKind::Modify
        | FileEventKind::Rescan
        | FileEventKind::Other => {
            if is_observable_fileprovider_path(previous, &event.path) {
                vec![event.path.clone()]
            } else {
                Vec::new()
            }
        }
    }
}

fn is_observable_fileprovider_path(
    previous: Option<&FileProviderStateSnapshot>,
    path: &Path,
) -> bool {
    if previous.is_some_and(|snapshot| snapshot.contains_path(path)) || provider_path_hint(path) {
        return true;
    }
    if !path.exists() {
        return false;
    }
    let hints = CloudHints::read(path);
    hints.source != "filesystem" || domain_for_path(path, &hints) != FileProviderDomain::Local
}

fn provider_path_hint(path: &Path) -> bool {
    if path_components(path)
        .iter()
        .any(|component| component == ICLOUD_DRIVE_COMPONENT)
    {
        return true;
    }
    let name = file_name_lower(path);
    is_evicted_placeholder_path(path) || name.contains("icloud") || name.contains("fileprovider")
}

impl FileProviderOperationReport {
    pub fn execute(path: impl AsRef<Path>, operation: FileProviderOperation) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let before = FileProviderStateReport::read_path(&path)?;
        let command = match operation {
            FileProviderOperation::Download => before.commands.download,
            FileProviderOperation::Evict => before.commands.evict,
        };
        if before.storage_state == CloudStorageState::Conflict {
            return Ok(Self::refused(
                path,
                operation,
                before,
                "provider-conflict-requires-resolution",
            ));
        }
        if command != CloudCommandState::Enabled {
            return Ok(Self::refused(
                path,
                operation,
                before,
                "operation-disabled-for-current-state",
            ));
        }
        if before.domain == FileProviderDomain::Local || !before.source_contains_native_resource() {
            return Ok(Self::refused(
                path,
                operation,
                before,
                "not-native-provider-backed",
            ));
        }

        let result = match operation {
            FileProviderOperation::Download => start_downloading_ubiquitous_item(&path),
            FileProviderOperation::Evict => evict_ubiquitous_item(&path),
        };
        match result.status {
            NativeFileProviderOperationStatus::Completed => {
                let after = FileProviderStateReport::read_path(&path).ok();
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
            | NativeFileProviderOperationStatus::UnsupportedPath
            | NativeFileProviderOperationStatus::Failed => Ok(Self {
                path,
                operation,
                disposition: FileProviderOperationDisposition::Failed,
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
}

impl FileProviderStateReport {
    pub fn read_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if !path.exists() && !is_evicted_placeholder_path(&path) {
            return Err(GfmError::io(&path, "path does not exist"));
        }
        Ok(Self::from_path(path))
    }

    pub fn from_path(path: PathBuf) -> Self {
        let hints = CloudHints::read(&path);
        Self::from_hints(path, hints)
    }

    pub fn from_path_with_native_identity(path: PathBuf) -> Self {
        let hints = CloudHints::read_with_identity(&path);
        Self::from_hints(path, hints)
    }

    fn from_hints(path: PathBuf, hints: CloudHints) -> Self {
        let domain = domain_for_path(&path, &hints);
        let storage_state = storage_state_for_path(&path, domain, &hints);
        let materialization = materialization_for_state(storage_state);
        let materialization_source = materialization_source_for_state(storage_state, &hints);
        let progress = progress_for_state(storage_state, &hints);
        let mut badges = badges_for_state(storage_state);
        badges.sort();
        badges.dedup();
        let commands = command_policy(domain, storage_state);

        Self {
            path,
            domain,
            storage_state,
            materialization,
            materialization_source,
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
        let storage_state = CloudStorageState::LocalOnly;
        let commands = CloudCommandPolicy::local();
        Self {
            path,
            domain: FileProviderDomain::Local,
            storage_state,
            materialization: CloudMaterialization::NotProviderBacked,
            materialization_source: CloudMaterializationSource::Filesystem,
            progress: CloudTransferProgress::idle("fileprovider-item-removed"),
            badges: Vec::new(),
            commands,
            offline: false,
            conflict: false,
            provider_identifier: None,
            source: "removed".to_string(),
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "fileprovider-state\t{}\tdomain={}\tstate={}\tmaterialization={}\tmaterialization-source={}\toffline={}\tconflict={}\tbadges={}\t{}\tdownload={}\tevict={}\treveal-conflict={}\tprovider={}\tsource={}\treason={}",
            self.path.display(),
            self.domain.as_str(),
            self.storage_state.as_str(),
            self.materialization.as_str(),
            self.materialization_source.as_str(),
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct CloudHints {
    native: NativeFileProviderResourceValues,
    native_identity: NativeFileProviderIdentity,
    xattrs: Vec<String>,
    provider_identifier: Option<String>,
    source: String,
}

impl CloudHints {
    fn read(path: &Path) -> Self {
        Self::read_with_optional_identity(path, None)
    }

    fn read_with_identity(path: &Path) -> Self {
        Self::read_with_optional_identity(path, Some(copy_fileprovider_identity(path)))
    }

    fn read_with_optional_identity(
        path: &Path,
        native_identity: Option<NativeFileProviderIdentity>,
    ) -> Self {
        let native = copy_fileprovider_resource_values(path);
        let native_identity = native_identity.unwrap_or_else(identity_not_queried);
        let mut xattrs = Vec::new();
        let mut provider_identifier = None;
        let mut sources = Vec::new();

        if native_has_fileprovider_values(&native) {
            sources.push("native-url-resource".to_string());
        }
        if native_identity.status == NativeFileProviderIdentityStatus::Available {
            sources.push("nsfileprovidermanager".to_string());
            provider_identifier = native_identity.domain_identifier.clone();
        }

        if let Ok(attrs) = xattr::list(path) {
            for attr in attrs {
                let attr = attr.to_string_lossy().to_string();
                if attr.contains("icloud")
                    || attr.contains("fileprovider")
                    || attr.contains("ubiquit")
                {
                    if provider_identifier.is_none() {
                        provider_identifier = provider_from_attr(path, &attr);
                    }
                    xattrs.push(attr);
                }
            }
            if !xattrs.is_empty() {
                sources.push("xattr".to_string());
            }
        }

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

        Self {
            native,
            native_identity,
            xattrs,
            provider_identifier,
            source: if sources.is_empty() {
                "filesystem".to_string()
            } else {
                sources.sort();
                sources.dedup();
                sources.join("+")
            },
        }
    }
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
        || file_name_lower(path).contains("icloud")
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
        || file_name_lower(path).contains("fileprovider")
    {
        FileProviderDomain::FileProvider
    } else {
        FileProviderDomain::Local
    }
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
    }

    let name = file_name_lower(path);
    let attr_blob = hints.xattrs.join("\n").to_ascii_lowercase();
    if name.contains("conflict") || attr_blob.contains("conflict") {
        CloudStorageState::Conflict
    } else if name.contains("offline") || attr_blob.contains("offline") {
        CloudStorageState::Offline
    } else if name.ends_with(".icloud")
        || name.contains("placeholder")
        || attr_blob.contains("placeholder")
        || attr_blob.contains("evict")
    {
        CloudStorageState::Evicted
    } else if name.contains("downloading") || attr_blob.contains("downloading") {
        CloudStorageState::Downloading
    } else if name.contains("uploading") || attr_blob.contains("uploading") {
        CloudStorageState::Uploading
    } else if name.contains("waiting") || attr_blob.contains("waiting") {
        CloudStorageState::Waiting
    } else if (domain == FileProviderDomain::FileProvider
        && !native_has_fileprovider_values(&hints.native))
        || (path_only_provider_hint(&hints.source) && hints.xattrs.is_empty())
    {
        CloudStorageState::Unknown
    } else if path.exists() {
        CloudStorageState::Downloaded
    } else {
        CloudStorageState::Unknown
    }
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
    if values.has_unresolved_conflicts == Some(true) {
        Some(CloudStorageState::Conflict)
    } else if values.is_downloading == Some(true) {
        Some(CloudStorageState::Downloading)
    } else if values.is_uploading == Some(true) {
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
            None if values
                .percent_uploaded_milli
                .is_some_and(|percent| percent < 100_000) =>
            {
                Some(CloudStorageState::Uploading)
            }
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
        CloudStorageState::Unknown => CloudMaterialization::Unknown,
    }
}

fn materialization_source_for_state(
    state: CloudStorageState,
    hints: &CloudHints,
) -> CloudMaterializationSource {
    if hints.native.is_ubiquitous == Some(true)
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
        CloudMaterializationSource::NativeUrlResourceUnavailable
    } else if hints.native_identity.status == NativeFileProviderIdentityStatus::Available
        && state == CloudStorageState::Unknown
    {
        CloudMaterializationSource::NativeFileProviderIdentityUnknown
    } else if state == CloudStorageState::Unknown
        && matches!(
            hints.native_identity.status,
            NativeFileProviderIdentityStatus::ProviderUnavailable
                | NativeFileProviderIdentityStatus::TimedOut
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
    } else if hints.source.contains("fixture-name") || hints.source.contains("icloud-extension") {
        CloudMaterializationSource::PathFallback
    } else if hints.source == "filesystem" {
        CloudMaterializationSource::Filesystem
    } else {
        CloudMaterializationSource::StateFallback
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
    }
}

fn command_policy(domain: FileProviderDomain, state: CloudStorageState) -> CloudCommandPolicy {
    if domain == FileProviderDomain::Local {
        return CloudCommandPolicy::local();
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
        CloudStorageState::LocalOnly => CloudCommandPolicy::local(),
    }
}

fn is_evicted_placeholder_path(path: &Path) -> bool {
    let name = file_name_lower(path);
    name.ends_with(".icloud") || name.contains("placeholder")
}

fn provider_from_attr(path: &Path, attr: &str) -> Option<String> {
    xattr::get(path, attr)
        .ok()
        .flatten()
        .and_then(|value| String::from_utf8(value).ok())
        .and_then(non_empty)
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

fn atomic_write_text(path: &Path, text: &str) -> Result<()> {
    let temporary = temporary_path(path);
    let mut file = File::create(&temporary).map_err(|err| GfmError::io(&temporary, err))?;
    if let Err(err) = file.write_all(text.as_bytes()) {
        let _ = fs::remove_file(&temporary);
        return Err(GfmError::io(&temporary, err));
    }
    if let Err(err) = file.sync_all() {
        let _ = fs::remove_file(&temporary);
        return Err(GfmError::io(&temporary, err));
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
    let Some(parent) = path.parent() else {
        return Ok(false);
    };
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
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn reports_path_only_icloud_file_as_unknown_without_native_evidence() {
        let root = unique_temp_dir();
        let path = root.join("Downloaded.icloud.md");
        fs::write(&path, "downloaded").unwrap();

        let report = FileProviderStateReport::read_path(&path).unwrap();

        assert_eq!(report.domain, FileProviderDomain::ICloudDrive);
        assert_eq!(report.storage_state, CloudStorageState::Unknown);
        assert_eq!(report.materialization, CloudMaterialization::Unknown);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::PathFallback
        );
        assert_eq!(report.progress.direction, CloudTransferDirection::Idle);
        assert_eq!(report.progress.percent_milli, None);
        assert!(!report.progress.complete);
        assert_eq!(
            report.progress.reason.as_deref(),
            Some("unknown-provider-state")
        );
        assert_eq!(report.badges, vec![CloudBadge::Waiting]);
        assert_eq!(report.commands.evict, CloudCommandState::Disabled);
        assert_eq!(report.commands.download, CloudCommandState::Disabled);
        assert_eq!(
            report.commands.reason.as_deref(),
            Some("unknown-provider-state")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_evicted_placeholder_with_download_command() {
        let root = unique_temp_dir();
        let path = root.join("Evicted.icloud-placeholder");
        fs::write(&path, "placeholder").unwrap();

        let report = FileProviderStateReport::read_path(&path).unwrap();

        assert_eq!(report.storage_state, CloudStorageState::Evicted);
        assert_eq!(report.progress.direction, CloudTransferDirection::Download);
        assert_eq!(report.progress.percent_milli, Some(0));
        assert!(!report.progress.requested);
        assert!(!report.progress.indeterminate);
        assert_eq!(report.badges, vec![CloudBadge::Cloud]);
        assert!(report.offline);
        assert_eq!(report.commands.download, CloudCommandState::Enabled);
        assert_eq!(report.commands.evict, CloudCommandState::Disabled);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_conflict_with_resolution_command() {
        let root = unique_temp_dir();
        let path = root.join("Conflict.icloud-conflict.md");
        fs::write(&path, "conflict").unwrap();

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
        assert_eq!(report.reason, "no-provider-conflict");

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
    fn fileprovider_state_invalidation_persists_current_provider_transitions() {
        let root = unique_temp_dir();
        let evicted = root.join("Remote.icloud-placeholder");
        fs::write(&evicted, "placeholder").unwrap();
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
    fn observed_fileprovider_invalidation_maps_events_to_provider_paths() {
        let root = unique_temp_dir();
        let evicted = root.join("Remote.icloud-placeholder");
        fs::write(&evicted, "placeholder").unwrap();
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
        assert_eq!(observed.paths, vec![evicted.clone()]);
        assert_eq!(observed.report.changes.len(), 1);
        assert!(observed.report.invalidate_icon);
        assert!(observed.report.invalidate_preview_memory);
        assert!(observed.report.invalidate_preview_disk);
        assert!(observed.report.invalidate_sidebar);
        assert!(observed.report.reindex_metadata);
        assert_eq!(snapshot.entries[0].path, evicted);
        assert_eq!(snapshot.entries[0].state, CloudStorageState::Evicted);
        assert!(observed
            .as_tsv()
            .contains("fileprovider-observed-invalidation\tevents=1\tpaths=1"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn observed_fileprovider_invalidation_preserves_snapshot_for_irrelevant_events() {
        let root = unique_temp_dir();
        let tracked = root.join("Remote.icloud-placeholder");
        fs::write(&tracked, "placeholder").unwrap();
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
        assert_eq!(change.current.storage_state, CloudStorageState::LocalOnly);
        assert_eq!(change.current.source, "removed");
        assert_eq!(
            change.current.progress.reason.as_deref(),
            Some("fileprovider-item-removed")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn observed_fileprovider_invalidation_preserves_unrelated_snapshot_entries() {
        let root = unique_temp_dir();
        let changed = root.join("Changed.icloud-placeholder");
        let untouched = root.join("Untouched.icloud-placeholder");
        fs::write(&changed, "placeholder").unwrap();
        fs::write(&untouched, "placeholder").unwrap();
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
        assert_eq!(report.domain, FileProviderDomain::ICloudDrive);
        assert_eq!(report.storage_state, CloudStorageState::Unknown);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn domain_report_does_not_claim_local_files_are_provider_backed() {
        let root = unique_temp_dir();
        let path = root.join("Local.md");
        fs::write(&path, "local").unwrap();

        let report = FileProviderDomainReport::read_path(&path).unwrap();

        assert_eq!(report.domain, FileProviderDomain::Local);
        assert_ne!(
            report.native_identity_status,
            NativeFileProviderIdentityStatus::Available
        );
        assert!(report.item_identifier.is_none());
        assert!(report.domain_identifier.is_none());
        assert!(report.as_tsv().starts_with("fileprovider-domain\t"));

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
    fn generic_provider_path_fallback_without_materialization_evidence_is_unknown() {
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
            provider_identifier: None,
            source: "fixture-name".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path.clone(), hints);

        assert_eq!(report.domain, FileProviderDomain::FileProvider);
        assert_eq!(report.storage_state, CloudStorageState::Unknown);
        assert_eq!(report.materialization, CloudMaterialization::Unknown);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::PathFallback
        );
        assert_eq!(
            report.commands.reason.as_deref(),
            Some("unknown-provider-state")
        );
        assert_eq!(
            report.progress.reason.as_deref(),
            Some("unknown-provider-state")
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
            Some("not-native-provider-backed")
        );
        assert_eq!(download.before.storage_state, CloudStorageState::Evicted);

        let evict = FileProviderOperationReport::execute(&downloaded, FileProviderOperation::Evict)
            .unwrap();
        assert_eq!(evict.disposition, FileProviderOperationDisposition::Refused);
        assert_eq!(
            evict.reason.as_deref(),
            Some("operation-disabled-for-current-state")
        );
        assert_eq!(evict.before.storage_state, CloudStorageState::Unknown);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operations_refuse_disabled_state_before_native_call() {
        let root = unique_temp_dir();
        let downloading = root.join("Downloading.icloud-downloading.md");
        fs::write(&downloading, "downloading").unwrap();

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
    fn invalidation_marks_provider_state_transitions() {
        let root = unique_temp_dir();
        let evicted = root.join("Remote.icloud-placeholder");
        fs::write(&evicted, "placeholder").unwrap();

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
            FileProviderInvalidationReport::evaluate(&downloaded, CloudStorageState::Unknown)
                .unwrap();

        assert!(!report.state_changed);
        assert!(!report.invalidate_icon);
        assert!(!report.invalidate_preview_memory);
        assert!(!report.invalidate_preview_disk);
        assert!(report.invalidate_sidebar);
        assert!(!report.reindex_metadata);
        assert_eq!(report.reason, "fileprovider-state-unchanged");

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
    fn parses_cloud_storage_state_for_operator_surfaces() {
        assert_eq!(
            CloudStorageState::parse("downloading").unwrap(),
            CloudStorageState::Downloading
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
    fn path_only_icloud_name_does_not_claim_materialized_without_native_evidence() {
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
            provider_identifier: None,
            source: "fixture-name".to_string(),
        };

        let report = FileProviderStateReport::from_hints(path, hints);

        assert_eq!(report.domain, FileProviderDomain::ICloudDrive);
        assert_eq!(report.storage_state, CloudStorageState::Unknown);
        assert_eq!(report.materialization, CloudMaterialization::Unknown);
        assert_eq!(
            report.materialization_source,
            CloudMaterializationSource::PathFallback
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
    }

    #[test]
    fn native_identity_unavailable_unknown_state_reports_typed_source() {
        let path = PathBuf::from("/tmp/Remote.fileprovider");
        let hints = CloudHints {
            native: native_values(),
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::TimedOut,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("NSFileProviderManager identity lookup timed out".to_string()),
            },
            xattrs: Vec::new(),
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
    }

    #[test]
    fn materialization_report_marks_path_fallback_placeholders() {
        let path = PathBuf::from("/tmp/Remote.icloud-placeholder");
        let hints = CloudHints {
            native: native_values(),
            native_identity: NativeFileProviderIdentity {
                status: NativeFileProviderIdentityStatus::NotQueried,
                item_identifier: None,
                domain_identifier: None,
                reason: Some("test fixture has no native manager identity".to_string()),
            },
            xattrs: Vec::new(),
            provider_identifier: None,
            source: "fixture-name+icloud-extension".to_string(),
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
            CloudMaterializationSource::PathFallback
        );
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
            status: NativeFileProviderStatus::Available,
            reason: None,
        }
    }
}
