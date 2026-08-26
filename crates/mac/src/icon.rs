use crate::{MacBridgeThreadPolicy, MacFramework, SupportEvaluation, SupportMatrix, SupportTier};
use gfm_types::{FileKind, FileRecord};
use std::path::Path;

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
    Hidden,
    Package,
    Tagged,
}

impl NativeIconBadge {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Alias => "alias",
            Self::Hidden => "hidden",
            Self::Package => "package",
            Self::Tagged => "tagged",
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

    pub fn for_record_with_evaluation(record: &FileRecord, evaluation: &SupportEvaluation) -> Self {
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
            descriptor: NativeIconDescriptor::for_record(record),
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

impl NativeIconDescriptor {
    pub fn for_record(record: &FileRecord) -> Self {
        let role = role_for_record(record);
        let provider = provider_for_role(role);
        let mut badges = badges_for_record(record);
        badges.sort();
        badges.dedup();
        let type_hint = type_hint_for_record(record);
        let cache_key = cache_key(role, &type_hint, &badges);

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

fn badges_for_record(record: &FileRecord) -> Vec<NativeIconBadge> {
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
    badges
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

fn cache_key(role: NativeIconRole, type_hint: &str, badges: &[NativeIconBadge]) -> String {
    let badges = badges
        .iter()
        .map(|badge| badge.as_str())
        .collect::<Vec<_>>()
        .join("+");
    if badges.is_empty() {
        format!("{}:{type_hint}", role.as_str())
    } else {
        format!("{}:{type_hint}:{badges}", role.as_str())
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
    use std::path::PathBuf;

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
    fn tsv_output_is_stable_for_cli_and_fozzy() {
        let record = record("Slides.key", FileKind::Directory);
        let descriptor = NativeIconDescriptor::for_record(&record);

        assert_eq!(
            descriptor.as_tsv(),
            "native-icon\tpackage\tlaunchservices-package-icon\tcom.apple.iwork.keynote.key\tpackage:com.apple.iwork.keynote.key:package\tbadges=package"
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
}
