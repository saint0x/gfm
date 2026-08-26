use gfm_mac_sys::{
    copy_fileprovider_resource_values, evict_ubiquitous_item, start_downloading_ubiquitous_item,
    NativeFileProviderOperationStatus, NativeFileProviderResourceValues,
    NativeUbiquitousDownloadingStatus,
};
use gfm_types::{GfmError, Result};
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
    pub progress: CloudTransferProgress,
    pub badges: Vec<CloudBadge>,
    pub commands: CloudCommandPolicy,
    pub offline: bool,
    pub conflict: bool,
    pub provider_identifier: Option<String>,
    pub source: String,
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
            reason: percent_milli
                .is_none()
                .then(|| "provider-progress-unavailable".to_string()),
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

impl FileProviderInvalidationReport {
    pub fn evaluate(
        path: impl AsRef<Path>,
        previous: CloudStorageState,
    ) -> Result<FileProviderInvalidationReport> {
        let path = path.as_ref().to_path_buf();
        let current = FileProviderStateReport::read_path(&path)?;
        let state_changed = previous != current.storage_state;
        let provider_visible = current.domain != FileProviderDomain::Local
            || previous != CloudStorageState::LocalOnly
            || !current.badges.is_empty();
        let invalidate_sidebar = provider_visible
            && matches!(
                current.storage_state,
                CloudStorageState::Downloaded
                    | CloudStorageState::Evicted
                    | CloudStorageState::Downloading
                    | CloudStorageState::Uploading
                    | CloudStorageState::Waiting
                    | CloudStorageState::Conflict
                    | CloudStorageState::Offline
                    | CloudStorageState::Unknown
            );
        let reason = if !provider_visible {
            "not-provider-visible"
        } else if state_changed {
            "fileprovider-state-changed"
        } else {
            "fileprovider-state-unchanged"
        };

        Ok(FileProviderInvalidationReport {
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
        })
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
        let domain = domain_for_path(&path, &hints);
        let storage_state = storage_state_for_path(&path, domain, &hints);
        let progress = progress_for_state(storage_state, &hints);
        let mut badges = badges_for_state(storage_state);
        badges.sort();
        badges.dedup();
        let commands = command_policy(domain, storage_state);

        Self {
            path,
            domain,
            storage_state,
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

    pub fn as_tsv(&self) -> String {
        format!(
            "fileprovider-state\t{}\tdomain={}\tstate={}\toffline={}\tconflict={}\tbadges={}\t{}\tdownload={}\tevict={}\treveal-conflict={}\tprovider={}\tsource={}\treason={}",
            self.path.display(),
            self.domain.as_str(),
            self.storage_state.as_str(),
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
    xattrs: Vec<String>,
    provider_identifier: Option<String>,
    source: String,
}

impl CloudHints {
    fn read(path: &Path) -> Self {
        let native = copy_fileprovider_resource_values(path);
        let mut xattrs = Vec::new();
        let mut provider_identifier = None;
        let mut sources = Vec::new();

        if native_has_fileprovider_values(&native) {
            sources.push("native-url-resource".to_string());
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

fn domain_for_path(path: &Path, hints: &CloudHints) -> FileProviderDomain {
    if hints.native.is_ubiquitous == Some(true)
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
    } else if hints
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

fn storage_state_for_path(
    path: &Path,
    domain: FileProviderDomain,
    hints: &CloudHints,
) -> CloudStorageState {
    if domain == FileProviderDomain::Local {
        return CloudStorageState::LocalOnly;
    }

    if hints.native.is_ubiquitous == Some(true) {
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
    } else if path.exists() {
        CloudStorageState::Downloaded
    } else {
        CloudStorageState::Unknown
    }
}

fn native_storage_state(values: &NativeFileProviderResourceValues) -> Option<CloudStorageState> {
    if values.has_unresolved_conflicts == Some(true) {
        Some(CloudStorageState::Conflict)
    } else if values.is_downloading == Some(true) {
        Some(CloudStorageState::Downloading)
    } else if values.is_uploading == Some(true) {
        Some(CloudStorageState::Uploading)
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
            None if values.is_uploaded == Some(false) => Some(CloudStorageState::Waiting),
            None if values.is_uploaded == Some(true) => Some(CloudStorageState::Downloaded),
            None => None,
        }
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
        || values.is_downloading.is_some()
        || values.is_uploading.is_some()
        || values.is_uploaded.is_some()
        || values.download_requested.is_some()
        || values.percent_downloaded_milli.is_some()
        || values.percent_uploaded_milli.is_some()
        || values.downloading_status.is_some()
}

fn escape_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
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
    use gfm_mac_sys::NativeFileProviderStatus;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn reports_downloaded_icloud_file_with_evict_command() {
        let root = unique_temp_dir();
        let path = root.join("Downloaded.icloud.md");
        fs::write(&path, "downloaded").unwrap();

        let report = FileProviderStateReport::read_path(&path).unwrap();

        assert_eq!(report.domain, FileProviderDomain::ICloudDrive);
        assert_eq!(report.storage_state, CloudStorageState::Downloaded);
        assert_eq!(report.progress.direction, CloudTransferDirection::Download);
        assert_eq!(report.progress.percent_milli, Some(100_000));
        assert!(report.progress.complete);
        assert_eq!(report.badges, vec![CloudBadge::AvailableOffline]);
        assert_eq!(report.commands.evict, CloudCommandState::Enabled);
        assert_eq!(report.commands.download, CloudCommandState::Disabled);

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
        assert_eq!(evict.reason.as_deref(), Some("not-native-provider-backed"));
        assert_eq!(evict.before.storage_state, CloudStorageState::Downloaded);

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
            FileProviderInvalidationReport::evaluate(&downloaded, CloudStorageState::Downloaded)
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
            xattrs: Vec::new(),
            provider_identifier: None,
            source: "native-url-resource".to_string(),
        };

        let domain = domain_for_path(&path, &hints);
        let state = storage_state_for_path(&path, domain, &hints);

        assert_eq!(domain, FileProviderDomain::ICloudDrive);
        assert_eq!(state, CloudStorageState::Conflict);
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
            is_downloading: None,
            is_uploading: None,
            is_uploaded: None,
            download_requested: None,
            percent_downloaded_milli: None,
            percent_uploaded_milli: None,
            downloading_status: None,
            status: NativeFileProviderStatus::Available,
            reason: None,
        }
    }
}
