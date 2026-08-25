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
    pub badges: Vec<CloudBadge>,
    pub commands: CloudCommandPolicy,
    pub offline: bool,
    pub conflict: bool,
    pub provider_identifier: Option<String>,
    pub source: String,
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
        let mut badges = badges_for_state(storage_state);
        badges.sort();
        badges.dedup();
        let commands = command_policy(domain, storage_state);

        Self {
            path,
            domain,
            storage_state,
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
            "fileprovider-state\t{}\tdomain={}\tstate={}\toffline={}\tconflict={}\tbadges={}\tdownload={}\tevict={}\treveal-conflict={}\tprovider={}\tsource={}\treason={}",
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CloudHints {
    xattrs: Vec<String>,
    provider_identifier: Option<String>,
    source: String,
}

impl CloudHints {
    fn read(path: &Path) -> Self {
        let mut xattrs = Vec::new();
        let mut provider_identifier = None;
        let mut sources = Vec::new();

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
    if path_components(path)
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

fn escape_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn unique_temp_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "gfm-fileprovider-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::SeqCst),
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
