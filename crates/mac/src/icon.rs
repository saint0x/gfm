use crate::{
    CloudBadge, CloudStorageState, FileProviderDomain, FileProviderInvalidationReport,
    FileProviderStateReport, MacBridgeThreadPolicy, MacFramework, SupportEvaluation, SupportMatrix,
    SupportTier,
};
use gfm_types::{FileKind, FileRecord};
use std::env;
use std::path::{Path, PathBuf};

const FINDER_INFO_XATTR: &str = "com.apple.FinderInfo";
const FINDER_FLAG_CUSTOM_ICON: u16 = 0x0400;
const CUSTOM_FOLDER_ICON_FILE: &str = "Icon\r";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeIconRole {
    Application,
    Folder,
    Package,
    Document,
    Symlink,
    Other,
}

impl NativeIconRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Application => "application",
            Self::Folder => "folder",
            Self::Package => "package",
            Self::Document => "document",
            Self::Symlink => "symlink",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeIconProvider {
    FinderCustomIcon,
    LaunchServicesApplicationIcon,
    LaunchServicesFolderIcon,
    LaunchServicesPackageIcon,
    LaunchServicesDocumentIcon,
    LaunchServicesSymlinkIcon,
    LaunchServicesGenericIcon,
}

impl NativeIconProvider {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FinderCustomIcon => "finder-custom-icon",
            Self::LaunchServicesApplicationIcon => "launchservices-application-icon",
            Self::LaunchServicesFolderIcon => "launchservices-folder-icon",
            Self::LaunchServicesPackageIcon => "launchservices-package-icon",
            Self::LaunchServicesDocumentIcon => "launchservices-document-icon",
            Self::LaunchServicesSymlinkIcon => "launchservices-symlink-icon",
            Self::LaunchServicesGenericIcon => "launchservices-generic-icon",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeIconBridgeDecision {
    UseNativeBridge,
    UseDescriptorFallback,
}

impl NativeIconBridgeDecision {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UseNativeBridge => "use-native-bridge",
            Self::UseDescriptorFallback => "use-descriptor-fallback",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NativeIconBadge {
    Alias,
    Cloud,
    CloudAvailableOffline,
    CloudConflict,
    CloudDownloading,
    CloudOffline,
    CloudUploading,
    CloudWaiting,
    Hidden,
    Package,
    Tagged,
    VolumeDiskImage,
    VolumeExternal,
    VolumeNetwork,
    VolumeOffline,
    VolumeReadOnly,
    VolumeRemovable,
    VolumeUnavailable,
}

impl NativeIconBadge {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Alias => "alias",
            Self::Cloud => "cloud",
            Self::CloudAvailableOffline => "cloud-available-offline",
            Self::CloudConflict => "cloud-conflict",
            Self::CloudDownloading => "cloud-downloading",
            Self::CloudOffline => "cloud-offline",
            Self::CloudUploading => "cloud-uploading",
            Self::CloudWaiting => "cloud-waiting",
            Self::Hidden => "hidden",
            Self::Package => "package",
            Self::Tagged => "tagged",
            Self::VolumeDiskImage => "volume-disk-image",
            Self::VolumeExternal => "volume-external",
            Self::VolumeNetwork => "volume-network",
            Self::VolumeOffline => "volume-offline",
            Self::VolumeReadOnly => "volume-read-only",
            Self::VolumeRemovable => "volume-removable",
            Self::VolumeUnavailable => "volume-unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeIconDescriptor {
    pub role: NativeIconRole,
    pub provider: NativeIconProvider,
    pub badges: Vec<NativeIconBadge>,
    pub type_hint: String,
    pub cache_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeIconBridgeContract {
    pub descriptor: NativeIconDescriptor,
    pub framework: MacFramework,
    pub thread_policy: MacBridgeThreadPolicy,
    pub support_tier: SupportTier,
    pub decision: NativeIconBridgeDecision,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeIconInvalidationReport {
    pub path: PathBuf,
    pub previous: CloudStorageState,
    pub current: CloudStorageState,
    pub previous_badges: Vec<NativeIconBadge>,
    pub current_badges: Vec<NativeIconBadge>,
    pub previous_cache_key: String,
    pub current_cache_key: String,
    pub invalidate_cache: bool,
    pub reason: String,
}

impl NativeIconBridgeContract {
    pub fn for_record(record: &FileRecord) -> Self {
        Self::for_record_with_evaluation(
            record,
            &SupportEvaluation {
                tier: SupportTier::Primary,
                reasons: Vec::new(),
            },
        )
    }

    pub fn for_record_on_host(record: &FileRecord, host: &crate::HostProfile) -> Self {
        let evaluation = SupportMatrix::default().evaluate(host);
        Self::for_record_with_evaluation(record, &evaluation)
    }

    pub fn for_record_on_host_with_volume(
        record: &FileRecord,
        host: &crate::HostProfile,
        volume: Option<&crate::VolumeDescriptor>,
    ) -> Self {
        let evaluation = SupportMatrix::default().evaluate(host);
        Self::for_record_with_evaluation_and_volume(record, &evaluation, volume)
    }

    pub fn for_record_with_evaluation(record: &FileRecord, evaluation: &SupportEvaluation) -> Self {
        Self::for_record_with_evaluation_and_volume(record, evaluation, None)
    }

    pub fn for_record_with_evaluation_and_volume(
        record: &FileRecord,
        evaluation: &SupportEvaluation,
        volume: Option<&crate::VolumeDescriptor>,
    ) -> Self {
        let decision = match evaluation.tier {
            SupportTier::Primary | SupportTier::Compatible => {
                NativeIconBridgeDecision::UseNativeBridge
            }
            SupportTier::Unsupported => NativeIconBridgeDecision::UseDescriptorFallback,
        };
        let reason = if evaluation.reasons.is_empty() {
            "host-supported".to_string()
        } else {
            evaluation.reasons.join("|")
        };

        Self {
            descriptor: NativeIconDescriptor::for_record_on_volume(record, volume),
            framework: MacFramework::LaunchServices,
            thread_policy: MacBridgeThreadPolicy::BackgroundSafe,
            support_tier: evaluation.tier,
            decision,
            reason,
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "native-icon-bridge\t{}\t{}\t{}\t{}\ttier={}\tdecision={}\treason={}",
            self.framework.as_str(),
            self.thread_policy.as_str(),
            self.descriptor.provider.as_str(),
            self.descriptor.cache_key,
            self.support_tier.as_str(),
            self.decision.as_str(),
            self.reason
        )
    }
}

impl NativeIconInvalidationReport {
    pub fn from_fileprovider(report: &FileProviderInvalidationReport) -> Self {
        let previous_badges = native_cloud_badges_for_state(report.previous);
        let current_badges = report
            .current
            .badges
            .iter()
            .copied()
            .map(cloud_badge)
            .collect::<Vec<_>>();
        let previous_cache_key = provider_badge_cache_key(report.previous, &previous_badges);
        let current_cache_key =
            provider_badge_cache_key(report.current.storage_state, &current_badges);
        let badge_changed = previous_badges != current_badges;
        let cache_key_changed = previous_cache_key != current_cache_key;
        let invalidate_cache = report.invalidate_icon && (badge_changed || cache_key_changed);
        let reason = if !report.invalidate_icon {
            "provider-did-not-invalidate-icon".to_string()
        } else if badge_changed {
            "native-icon-badges-changed".to_string()
        } else if cache_key_changed {
            "native-icon-cache-key-changed".to_string()
        } else {
            report.reason.to_string()
        };

        Self {
            path: report.path.clone(),
            previous: report.previous,
            current: report.current.storage_state,
            previous_badges,
            current_badges,
            previous_cache_key,
            current_cache_key,
            invalidate_cache,
            reason,
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "native-icon-invalidation\t{}\tprevious={}\tcurrent={}\tprevious-badges={}\tcurrent-badges={}\tprevious-cache={}\tcurrent-cache={}\tinvalidate-cache={}\treason={}",
            escape_path_field(&self.path),
            self.previous.as_str(),
            self.current.as_str(),
            native_badges_tsv(&self.previous_badges),
            native_badges_tsv(&self.current_badges),
            self.previous_cache_key,
            self.current_cache_key,
            self.invalidate_cache,
            self.reason
        )
    }
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

impl NativeIconDescriptor {
    pub fn for_record(record: &FileRecord) -> Self {
        Self::for_record_on_volume(record, None)
    }

    pub fn for_record_on_volume(
        record: &FileRecord,
        volume: Option<&crate::VolumeDescriptor>,
    ) -> Self {
        let role = role_for_record(record);
        let provider = provider_for_record(record, role);
        let mut badges = badges_for_record_on_volume(record, volume);
        badges.sort();
        badges.dedup();
        let type_hint = type_hint_for_record(record);
        let cache_key = cache_key(record, role, provider, &type_hint, &badges);

        Self {
            role,
            provider,
            badges,
            type_hint,
            cache_key,
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "native-icon\t{}\t{}\t{}\t{}\tbadges={}",
            self.role.as_str(),
            self.provider.as_str(),
            self.type_hint,
            self.cache_key,
            self.badges
                .iter()
                .map(|badge| badge.as_str())
                .collect::<Vec<_>>()
                .join(",")
        )
    }
}

fn role_for_record(record: &FileRecord) -> NativeIconRole {
    if record.kind == FileKind::Symlink {
        NativeIconRole::Symlink
    } else if is_application(record) {
        NativeIconRole::Application
    } else if is_package(record) {
        NativeIconRole::Package
    } else {
        match record.kind {
            FileKind::Directory => NativeIconRole::Folder,
            FileKind::File => NativeIconRole::Document,
            FileKind::Symlink => NativeIconRole::Symlink,
            FileKind::Other => NativeIconRole::Other,
        }
    }
}

fn provider_for_record(record: &FileRecord, role: NativeIconRole) -> NativeIconProvider {
    if has_finder_custom_icon(record) {
        return NativeIconProvider::FinderCustomIcon;
    }
    provider_for_role(role)
}

fn provider_for_role(role: NativeIconRole) -> NativeIconProvider {
    match role {
        NativeIconRole::Application => NativeIconProvider::LaunchServicesApplicationIcon,
        NativeIconRole::Folder => NativeIconProvider::LaunchServicesFolderIcon,
        NativeIconRole::Package => NativeIconProvider::LaunchServicesPackageIcon,
        NativeIconRole::Document => NativeIconProvider::LaunchServicesDocumentIcon,
        NativeIconRole::Symlink => NativeIconProvider::LaunchServicesSymlinkIcon,
        NativeIconRole::Other => NativeIconProvider::LaunchServicesGenericIcon,
    }
}

fn badges_for_record_on_volume(
    record: &FileRecord,
    volume: Option<&crate::VolumeDescriptor>,
) -> Vec<NativeIconBadge> {
    let mut badges = Vec::new();
    if record.kind == FileKind::Symlink {
        badges.push(NativeIconBadge::Alias);
    }
    if record.hidden {
        badges.push(NativeIconBadge::Hidden);
    }
    if !record.tags.is_empty() {
        badges.push(NativeIconBadge::Tagged);
    }
    if is_package(record) {
        badges.push(NativeIconBadge::Package);
    }
    if let Ok(cloud) = FileProviderStateReport::from_path(record.path.clone()) {
        if cloud.domain != FileProviderDomain::Local {
            badges.extend(cloud.badges.iter().copied().map(cloud_badge));
        }
    }
    if let Some(volume) = volume.filter(|volume| record_is_volume_root(record, volume)) {
        badges.extend(volume_badges(volume));
    }
    badges
}

fn record_is_volume_root(record: &FileRecord, volume: &crate::VolumeDescriptor) -> bool {
    comparable_icon_path(&record.path) == comparable_icon_path(&volume.path)
}

fn comparable_icon_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    env::current_dir()
        .map(|cwd| cwd.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

fn volume_badges(volume: &crate::VolumeDescriptor) -> Vec<NativeIconBadge> {
    let mut badges = match volume.kind {
        crate::VolumeKind::External => vec![NativeIconBadge::VolumeExternal],
        crate::VolumeKind::Removable => vec![NativeIconBadge::VolumeRemovable],
        crate::VolumeKind::Network => vec![NativeIconBadge::VolumeNetwork],
        crate::VolumeKind::DiskImage => vec![NativeIconBadge::VolumeDiskImage],
        crate::VolumeKind::System | crate::VolumeKind::Internal | crate::VolumeKind::Unknown => {
            Vec::new()
        }
    };
    if volume.read_only {
        badges.push(NativeIconBadge::VolumeReadOnly);
    }
    if volume.mount_state != crate::MountState::Mounted || volume.reachable == Some(false) {
        badges.push(NativeIconBadge::VolumeOffline);
    }
    if volume.platform_state_unavailable() {
        badges.push(NativeIconBadge::VolumeUnavailable);
    }
    badges
}

fn cloud_badge(badge: CloudBadge) -> NativeIconBadge {
    match badge {
        CloudBadge::AvailableOffline => NativeIconBadge::CloudAvailableOffline,
        CloudBadge::Cloud => NativeIconBadge::Cloud,
        CloudBadge::Downloading => NativeIconBadge::CloudDownloading,
        CloudBadge::Uploading => NativeIconBadge::CloudUploading,
        CloudBadge::Waiting => NativeIconBadge::CloudWaiting,
        CloudBadge::Conflict => NativeIconBadge::CloudConflict,
        CloudBadge::Offline => NativeIconBadge::CloudOffline,
    }
}

fn native_cloud_badges_for_state(state: CloudStorageState) -> Vec<NativeIconBadge> {
    match state {
        CloudStorageState::LocalOnly => Vec::new(),
        CloudStorageState::Downloaded => vec![NativeIconBadge::CloudAvailableOffline],
        CloudStorageState::Evicted => vec![NativeIconBadge::Cloud],
        CloudStorageState::Downloading => {
            vec![NativeIconBadge::Cloud, NativeIconBadge::CloudDownloading]
        }
        CloudStorageState::Uploading => vec![NativeIconBadge::CloudUploading],
        CloudStorageState::Waiting | CloudStorageState::Unknown => {
            vec![NativeIconBadge::CloudWaiting]
        }
        CloudStorageState::Conflict => vec![NativeIconBadge::CloudConflict],
        CloudStorageState::Offline => vec![NativeIconBadge::CloudOffline],
        CloudStorageState::Removed => Vec::new(),
    }
}

fn provider_badge_cache_key(state: CloudStorageState, badges: &[NativeIconBadge]) -> String {
    format!(
        "fileprovider:{}:{}",
        state.as_str(),
        native_badges_tsv(badges)
    )
}

fn native_badges_tsv(badges: &[NativeIconBadge]) -> String {
    badges
        .iter()
        .map(|badge| badge.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

fn type_hint_for_record(record: &FileRecord) -> String {
    match record.kind {
        FileKind::Directory if is_application(record) => "com.apple.application-bundle".to_string(),
        FileKind::Directory if is_package(record) => package_type_hint(&record.path),
        FileKind::Directory => "public.folder".to_string(),
        FileKind::File => record
            .extension()
            .map(|extension| format!("extension:{}", extension.to_ascii_lowercase()))
            .unwrap_or_else(|| "public.data".to_string()),
        FileKind::Symlink => "public.symlink".to_string(),
        FileKind::Other => "public.item".to_string(),
    }
}

fn package_type_hint(path: &Path) -> String {
    match extension(path).as_deref() {
        Some("app") => "com.apple.application-bundle".to_string(),
        Some("framework") => "com.apple.framework".to_string(),
        Some("xcodeproj") => "com.apple.xcode.project".to_string(),
        Some("xcworkspace") => "com.apple.xcode.workspace".to_string(),
        Some("photoslibrary") => "com.apple.photos-library".to_string(),
        Some("pages") => "com.apple.iwork.pages.pages".to_string(),
        Some("numbers") => "com.apple.iwork.numbers.numbers".to_string(),
        Some("key") => "com.apple.iwork.keynote.key".to_string(),
        Some(value) => format!("package-extension:{value}"),
        None => "com.apple.package".to_string(),
    }
}

fn cache_key(
    record: &FileRecord,
    role: NativeIconRole,
    provider: NativeIconProvider,
    type_hint: &str,
    badges: &[NativeIconBadge],
) -> String {
    let badges = badges
        .iter()
        .map(|badge| badge.as_str())
        .collect::<Vec<_>>()
        .join("+");
    if provider == NativeIconProvider::FinderCustomIcon {
        let identity = format!(
            "{}:{}:{:016x}",
            record.id.volume.0, record.id.node, record.xattrs_digest
        );
        if badges.is_empty() {
            return format!("custom:{identity}:{}:{type_hint}", role.as_str());
        }
        return format!("custom:{identity}:{}:{type_hint}:{badges}", role.as_str());
    }
    if badges.is_empty() {
        format!("{}:{type_hint}", role.as_str())
    } else {
        format!("{}:{type_hint}:{badges}", role.as_str())
    }
}

fn has_finder_custom_icon(record: &FileRecord) -> bool {
    finder_info_has_custom_icon(&record.path)
        || (record.kind == FileKind::Directory
            && record.path.join(CUSTOM_FOLDER_ICON_FILE).try_exists().ok() == Some(true))
}

fn finder_info_has_custom_icon(path: &Path) -> bool {
    let Some(raw) = xattr::get(path, FINDER_INFO_XATTR).ok().flatten() else {
        return false;
    };
    finder_info_flags(&raw) & FINDER_FLAG_CUSTOM_ICON != 0
}

fn finder_info_flags(raw: &[u8]) -> u16 {
    if raw.len() < 10 {
        0
    } else {
        u16::from_be_bytes([raw[8], raw[9]])
    }
}

fn is_application(record: &FileRecord) -> bool {
    record.kind == FileKind::Directory && extension(&record.path).as_deref() == Some("app")
}

fn is_package(record: &FileRecord) -> bool {
    record.kind == FileKind::Directory
        && matches!(
            extension(&record.path).as_deref(),
            Some(
                "app"
                    | "appex"
                    | "bundle"
                    | "framework"
                    | "key"
                    | "numbers"
                    | "pages"
                    | "photoslibrary"
                    | "playground"
                    | "rtfd"
                    | "xcodeproj"
                    | "xcworkspace"
            )
        )
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_types::{FileId, VolumeId};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn resolves_application_icon_descriptor_with_package_badge() {
        let mut record = record("GFM.app", FileKind::Directory);
        record.tags.push("Important".to_string());

        let descriptor = NativeIconDescriptor::for_record(&record);

        assert_eq!(descriptor.role, NativeIconRole::Application);
        assert_eq!(
            descriptor.provider,
            NativeIconProvider::LaunchServicesApplicationIcon
        );
        assert_eq!(descriptor.type_hint, "com.apple.application-bundle");
        assert_eq!(
            descriptor.badges,
            vec![NativeIconBadge::Package, NativeIconBadge::Tagged]
        );
    }

    #[test]
    fn resolves_document_icon_descriptor_from_extension() {
        let record = record("Report.PDF", FileKind::File);

        let descriptor = NativeIconDescriptor::for_record(&record);

        assert_eq!(descriptor.role, NativeIconRole::Document);
        assert_eq!(
            descriptor.provider,
            NativeIconProvider::LaunchServicesDocumentIcon
        );
        assert_eq!(descriptor.type_hint, "extension:pdf");
        assert_eq!(descriptor.cache_key, "document:extension:pdf");
    }

    #[test]
    fn resolves_symlink_icon_descriptor_with_alias_badge() {
        let record = record("Latest", FileKind::Symlink);

        let descriptor = NativeIconDescriptor::for_record(&record);

        assert_eq!(descriptor.role, NativeIconRole::Symlink);
        assert_eq!(descriptor.badges, vec![NativeIconBadge::Alias]);
        assert_eq!(descriptor.type_hint, "public.symlink");
    }

    #[test]
    fn path_only_icloud_items_do_not_carry_cloud_badges_without_provider_evidence() {
        let path = temp_path("gfm-native-path-only-icon", "icloud.md");
        fs::write(&path, "path-only iCloud hint").unwrap();
        let mut record = record("MaybeDownloaded.icloud.md", FileKind::File);
        record.path = path.clone();

        let descriptor = NativeIconDescriptor::for_record(&record);

        assert!(descriptor.badges.is_empty());
        assert_eq!(descriptor.cache_key, "document:extension:md");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn evicted_icloud_placeholders_carry_cloud_badge() {
        let path = temp_path("gfm-native-evicted-icon", "icloud-placeholder");
        fs::write(&path, "placeholder").unwrap();
        xattr::set(&path, "com.apple.icloud.placeholder", b"1").unwrap();
        let mut record = record("Remote.icloud-placeholder", FileKind::File);
        record.path = path.clone();

        let descriptor = NativeIconDescriptor::for_record(&record);

        assert_eq!(descriptor.badges, vec![NativeIconBadge::Cloud]);
        assert_eq!(
            descriptor.cache_key,
            "document:extension:icloud-placeholder:cloud"
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn in_flight_icloud_items_carry_progress_badge() {
        let path = temp_path("gfm-native-downloading-icon", "icloud-downloading.png");
        fs::write(&path, "downloading").unwrap();
        xattr::set(&path, "com.apple.fileprovider.state", b"downloading").unwrap();
        let mut record = record("Asset.icloud-downloading.png", FileKind::File);
        record.path = path.clone();

        let descriptor = NativeIconDescriptor::for_record(&record);

        assert_eq!(
            descriptor.badges,
            vec![NativeIconBadge::Cloud, NativeIconBadge::CloudDownloading]
        );
        assert_eq!(
            descriptor.cache_key,
            "document:extension:png:cloud+cloud-downloading"
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn volume_roots_carry_volume_badges_in_cache_key() {
        let root = temp_path("gfm-native-volume-icon", "network");
        fs::create_dir_all(&root).unwrap();
        let mut volume = crate::VolumeDescriptor::for_path(&root).unwrap();
        volume.kind = crate::VolumeKind::Network;
        volume.network = true;
        volume.local = Some(false);
        volume.reachable = Some(true);
        let mut record = record("network", FileKind::Directory);
        record.path = root.clone();

        let descriptor = NativeIconDescriptor::for_record_on_volume(&record, Some(&volume));

        assert_eq!(descriptor.role, NativeIconRole::Folder);
        assert_eq!(descriptor.badges, vec![NativeIconBadge::VolumeNetwork]);
        assert_eq!(descriptor.cache_key, "folder:public.folder:volume-network");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn non_root_records_do_not_inherit_volume_badges() {
        let root = temp_path("gfm-native-volume-child-icon", "network");
        fs::create_dir_all(&root).unwrap();
        let child = root.join("Report.pdf");
        fs::write(&child, "pdf").unwrap();
        let mut volume = crate::VolumeDescriptor::for_path(&root).unwrap();
        volume.kind = crate::VolumeKind::Network;
        volume.network = true;
        volume.local = Some(false);
        let mut record = record("Report.pdf", FileKind::File);
        record.path = child.clone();

        let descriptor = NativeIconDescriptor::for_record_on_volume(&record, Some(&volume));

        assert!(descriptor.badges.is_empty());
        assert_eq!(descriptor.cache_key, "document:extension:pdf");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tsv_output_is_stable_for_cli_and_fozzy() {
        let record = record("Slides.key", FileKind::Directory);
        let descriptor = NativeIconDescriptor::for_record(&record);

        assert_eq!(
            descriptor.as_tsv(),
            "native-icon\tpackage\tlaunchservices-package-icon\tcom.apple.iwork.keynote.key\tpackage:com.apple.iwork.keynote.key:package\tbadges=package"
        );
    }

    #[test]
    fn finderinfo_custom_icon_uses_per_file_custom_cache_key() {
        let path = temp_path("gfm-native-custom-icon", "app");
        fs::create_dir_all(&path).unwrap();
        let mut finder_info = [0u8; 32];
        finder_info[8..10].copy_from_slice(&FINDER_FLAG_CUSTOM_ICON.to_be_bytes());
        xattr::set(&path, FINDER_INFO_XATTR, &finder_info).unwrap();
        let mut record = record("Custom.app", FileKind::Directory);
        record.path = path.clone();
        record.xattrs_digest = 0x1234;

        let descriptor = NativeIconDescriptor::for_record(&record);

        assert_eq!(descriptor.provider, NativeIconProvider::FinderCustomIcon);
        assert_eq!(descriptor.role, NativeIconRole::Application);
        assert_eq!(
            descriptor.cache_key,
            "custom:1:1:0000000000001234:application:com.apple.application-bundle:package"
        );
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn folder_icon_resource_uses_custom_icon_provider() {
        let path = temp_path("gfm-native-folder-icon", "folder");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join(CUSTOM_FOLDER_ICON_FILE), b"icon").unwrap();
        let mut record = record("CustomFolder", FileKind::Directory);
        record.path = path.clone();
        record.xattrs_digest = 0xabcd;

        let descriptor = NativeIconDescriptor::for_record(&record);

        assert_eq!(descriptor.provider, NativeIconProvider::FinderCustomIcon);
        assert_eq!(descriptor.role, NativeIconRole::Folder);
        assert_eq!(
            descriptor.cache_key,
            "custom:1:1:000000000000abcd:folder:public.folder"
        );
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn folder_icon_resource_ignores_unprobeable_custom_icon_child() {
        let path = temp_path("gfm-native-folder-icon-unprobeable", "folder");
        fs::create_dir_all(&path).unwrap();
        let mut record = record("PlainFolder", FileKind::Directory);
        record.path = path.join("Icon\r-unavailable".repeat(16));

        let descriptor = NativeIconDescriptor::for_record(&record);

        assert_eq!(
            descriptor.provider,
            NativeIconProvider::LaunchServicesFolderIcon
        );
        assert_eq!(descriptor.role, NativeIconRole::Folder);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn fileprovider_icon_invalidation_tracks_native_badge_cache_changes() {
        let report = fileprovider_report(
            CloudStorageState::Downloaded,
            CloudStorageState::Evicted,
            vec![CloudBadge::Cloud],
            true,
        );

        let invalidation = NativeIconInvalidationReport::from_fileprovider(&report);

        assert!(invalidation.invalidate_cache);
        assert_eq!(
            invalidation.previous_badges,
            vec![NativeIconBadge::CloudAvailableOffline]
        );
        assert_eq!(invalidation.current_badges, vec![NativeIconBadge::Cloud]);
        assert_eq!(invalidation.reason, "native-icon-badges-changed");
        assert_eq!(
            invalidation.as_tsv(),
            "native-icon-invalidation\t/tmp/Remote.icloud-placeholder\tprevious=downloaded\tcurrent=evicted\tprevious-badges=cloud-available-offline\tcurrent-badges=cloud\tprevious-cache=fileprovider:downloaded:cloud-available-offline\tcurrent-cache=fileprovider:evicted:cloud\tinvalidate-cache=true\treason=native-icon-badges-changed"
        );
    }

    #[test]
    fn native_icon_invalidation_tsv_escapes_control_characters_in_path() {
        let mut report = fileprovider_report(
            CloudStorageState::Downloaded,
            CloudStorageState::Evicted,
            vec![CloudBadge::Cloud],
            true,
        );
        report.path = PathBuf::from("/tmp/Reports\tQ3\nDraft\rIcon.icloud");

        let invalidation = NativeIconInvalidationReport::from_fileprovider(&report);
        let tsv = invalidation.as_tsv();

        assert_eq!(tsv.lines().count(), 1, "{tsv}");
        assert!(
            tsv.contains("Reports\\tQ3\\nDraft\\rIcon.icloud\tprevious=downloaded\t"),
            "{tsv}"
        );
        assert_eq!(tsv.split('\t').count(), 10, "{tsv}");
    }

    #[test]
    fn fileprovider_icon_invalidation_respects_provider_noop() {
        let report = fileprovider_report(
            CloudStorageState::Downloaded,
            CloudStorageState::Downloaded,
            vec![CloudBadge::AvailableOffline],
            false,
        );

        let invalidation = NativeIconInvalidationReport::from_fileprovider(&report);

        assert!(!invalidation.invalidate_cache);
        assert_eq!(
            invalidation.reason,
            "provider-did-not-invalidate-icon".to_string()
        );
    }

    #[test]
    fn bridge_contract_uses_native_launchservices_on_supported_hosts() {
        let record = record("Report.PDF", FileKind::File);
        let contract = NativeIconBridgeContract::for_record_with_evaluation(
            &record,
            &SupportEvaluation {
                tier: SupportTier::Compatible,
                reasons: Vec::new(),
            },
        );

        assert_eq!(contract.framework, MacFramework::LaunchServices);
        assert_eq!(
            contract.thread_policy,
            MacBridgeThreadPolicy::BackgroundSafe
        );
        assert_eq!(contract.decision, NativeIconBridgeDecision::UseNativeBridge);
        assert_eq!(contract.reason, "host-supported");
        assert_eq!(
            contract.as_tsv(),
            "native-icon-bridge\tlaunchservices\tbackground-safe\tlaunchservices-document-icon\tdocument:extension:pdf\ttier=compatible\tdecision=use-native-bridge\treason=host-supported"
        );
    }

    #[test]
    fn bridge_contract_falls_back_to_descriptor_on_unsupported_hosts() {
        let record = record("GFM.app", FileKind::Directory);
        let contract = NativeIconBridgeContract::for_record_with_evaluation(
            &record,
            &SupportEvaluation {
                tier: SupportTier::Unsupported,
                reasons: vec!["macOS 13.6.0 is below minimum 14.0.0".to_string()],
            },
        );

        assert_eq!(
            contract.decision,
            NativeIconBridgeDecision::UseDescriptorFallback
        );
        assert_eq!(contract.descriptor.role, NativeIconRole::Application);
        assert_eq!(contract.reason, "macOS 13.6.0 is below minimum 14.0.0");
    }

    fn record(name: &str, kind: FileKind) -> FileRecord {
        FileRecord {
            id: FileId::new(VolumeId(1), 1),
            parent: None,
            path: PathBuf::from("/tmp").join(name),
            name: name.to_string(),
            kind,
            len: 0,
            mode: 0,
            owner: 0,
            group: 0,
            xattrs_digest: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: name.starts_with('.'),
            tags: Vec::new(),
            finder_comment: None,
        }
    }

    fn temp_path(prefix: &str, extension: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{nanos}.{extension}",
            std::process::id()
        ))
    }

    fn fileprovider_report(
        previous: CloudStorageState,
        current: CloudStorageState,
        badges: Vec<CloudBadge>,
        invalidate_icon: bool,
    ) -> FileProviderInvalidationReport {
        FileProviderInvalidationReport {
            path: PathBuf::from("/tmp/Remote.icloud-placeholder"),
            previous,
            current: FileProviderStateReport {
                path: PathBuf::from("/tmp/Remote.icloud-placeholder"),
                domain: FileProviderDomain::ICloudDrive,
                storage_state: current,
                materialization: crate::CloudMaterialization::RemotePlaceholder,
                materialization_source: crate::CloudMaterializationSource::StateFallback,
                materialization_confidence: crate::CloudMaterializationConfidence::StateFallback,
                materialization_reason: Some("test".to_string()),
                progress: crate::CloudTransferProgress {
                    direction: crate::CloudTransferDirection::Idle,
                    percent_milli: None,
                    requested: false,
                    complete: false,
                    indeterminate: false,
                    source: "test",
                    reason: Some("test".to_string()),
                },
                badges,
                commands: crate::CloudCommandPolicy {
                    download: crate::CloudCommandState::Hidden,
                    evict: crate::CloudCommandState::Hidden,
                    reveal_conflict: crate::CloudCommandState::Hidden,
                    reason: None,
                },
                offline: false,
                conflict: false,
                provider_identifier: None,
                source: "test".to_string(),
            },
            state_changed: previous != current,
            invalidate_icon,
            invalidate_preview_memory: false,
            invalidate_preview_disk: false,
            invalidate_sidebar: false,
            reindex_metadata: false,
            reason: if previous == current {
                "fileprovider-state-unchanged"
            } else {
                "fileprovider-state-changed"
            },
        }
    }
}
