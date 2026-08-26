#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MacFramework {
    AppKit,
    Foundation,
    CoreServices,
    LaunchServices,
    QuickLook,
    FileEvents,
    Security,
    DiskArbitration,
    FileProvider,
    Spotlight,
}

impl MacFramework {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AppKit => "appkit",
            Self::Foundation => "foundation",
            Self::CoreServices => "coreservices",
            Self::LaunchServices => "launchservices",
            Self::QuickLook => "quicklook",
            Self::FileEvents => "file-events",
            Self::Security => "security",
            Self::DiskArbitration => "diskarbitration",
            Self::FileProvider => "fileprovider",
            Self::Spotlight => "spotlight",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MacBridgeStatus {
    Implemented,
    Required,
}

impl MacBridgeStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Implemented => "implemented",
            Self::Required => "required",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacBridgeThreadPolicy {
    MainThread,
    BackgroundSafe,
    DedicatedWorker,
}

impl MacBridgeThreadPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MainThread => "main-thread",
            Self::BackgroundSafe => "background-safe",
            Self::DedicatedWorker => "dedicated-worker",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacBridgeSpec {
    pub id: &'static str,
    pub framework: MacFramework,
    pub owner: &'static str,
    pub boundary: &'static str,
    pub thread_policy: MacBridgeThreadPolicy,
    pub status: MacBridgeStatus,
}

impl MacBridgeSpec {
    pub const fn new(
        id: &'static str,
        framework: MacFramework,
        owner: &'static str,
        boundary: &'static str,
        thread_policy: MacBridgeThreadPolicy,
        status: MacBridgeStatus,
    ) -> Self {
        Self {
            id,
            framework,
            owner,
            boundary,
            thread_policy,
            status,
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "bridge\t{}\t{}\t{}\t{}\t{}\t{}",
            self.id,
            self.framework.as_str(),
            self.owner,
            self.boundary,
            self.thread_policy.as_str(),
            self.status.as_str()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacBridgeContract {
    pub bridges: Vec<MacBridgeSpec>,
}

impl MacBridgeContract {
    pub fn finder_required() -> Self {
        let mut bridges = vec![
            MacBridgeSpec::new(
                "appkit-window-shell",
                MacFramework::AppKit,
                "crates/ui",
                "application-window-menu-activation",
                MacBridgeThreadPolicy::MainThread,
                MacBridgeStatus::Implemented,
            ),
            MacBridgeSpec::new(
                "foundation-host-profile",
                MacFramework::Foundation,
                "crates/mac",
                "sw-vers-uname-sysctl-host-profile",
                MacBridgeThreadPolicy::BackgroundSafe,
                MacBridgeStatus::Implemented,
            ),
            MacBridgeSpec::new(
                "security-permission-readiness",
                MacFramework::Security,
                "crates/mac",
                "tcc-aware-readable-scope-probes",
                MacBridgeThreadPolicy::BackgroundSafe,
                MacBridgeStatus::Implemented,
            ),
            MacBridgeSpec::new(
                "corefoundation-security-bookmarks",
                MacFramework::Security,
                "crates/mac-sys",
                "security-scoped-bookmark-create-resolve-access",
                MacBridgeThreadPolicy::BackgroundSafe,
                MacBridgeStatus::Implemented,
            ),
            MacBridgeSpec::new(
                "fsevents-file-event-stream",
                MacFramework::FileEvents,
                "crates/mac",
                "typed-create-modify-remove-rename-rescan-events",
                MacBridgeThreadPolicy::DedicatedWorker,
                MacBridgeStatus::Implemented,
            ),
            MacBridgeSpec::new(
                "coreservices-kind-string",
                MacFramework::CoreServices,
                "crates/mac",
                "launchservices-localized-kind-string",
                MacBridgeThreadPolicy::BackgroundSafe,
                MacBridgeStatus::Implemented,
            ),
            MacBridgeSpec::new(
                "coreservices-finder-metadata",
                MacFramework::CoreServices,
                "crates/mac",
                "localized-display-name-extension-hiding-finderinfo-aliases",
                MacBridgeThreadPolicy::BackgroundSafe,
                MacBridgeStatus::Required,
            ),
            MacBridgeSpec::new(
                "launchservices-icons-and-packages",
                MacFramework::LaunchServices,
                "crates/mac",
                "native-icons-bundle-identities-package-classification",
                MacBridgeThreadPolicy::BackgroundSafe,
                MacBridgeStatus::Required,
            ),
            MacBridgeSpec::new(
                "quicklook-preview",
                MacFramework::QuickLook,
                "crates/preview",
                "quicklook-preview-controller-thumbnail-generator",
                MacBridgeThreadPolicy::MainThread,
                MacBridgeStatus::Required,
            ),
            MacBridgeSpec::new(
                "diskarbitration-volumes",
                MacFramework::DiskArbitration,
                "crates/mac",
                "mount-unmount-eject-volume-capacity",
                MacBridgeThreadPolicy::DedicatedWorker,
                MacBridgeStatus::Required,
            ),
            MacBridgeSpec::new(
                "fileprovider-icloud-state",
                MacFramework::FileProvider,
                "crates/mac",
                "icloud-download-evict-conflict-offline-state",
                MacBridgeThreadPolicy::BackgroundSafe,
                MacBridgeStatus::Required,
            ),
            MacBridgeSpec::new(
                "spotlight-metadata-reconciliation",
                MacFramework::Spotlight,
                "crates/index",
                "metadata-import-without-primary-correctness-dependency",
                MacBridgeThreadPolicy::BackgroundSafe,
                MacBridgeStatus::Implemented,
            ),
        ];
        bridges.sort_by_key(|bridge| (bridge.status, bridge.framework, bridge.id));
        Self { bridges }
    }

    pub fn implemented_count(&self) -> usize {
        self.bridges
            .iter()
            .filter(|bridge| bridge.status == MacBridgeStatus::Implemented)
            .count()
    }

    pub fn required_count(&self) -> usize {
        self.bridges
            .iter()
            .filter(|bridge| bridge.status == MacBridgeStatus::Required)
            .count()
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = vec![format!(
            "mac-bridges\timplemented={}\trequired={}\ttotal={}",
            self.implemented_count(),
            self.required_count(),
            self.bridges.len()
        )];
        lines.extend(self.bridges.iter().map(MacBridgeSpec::as_tsv));
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_contract_separates_implemented_from_required_surfaces() {
        let contract = MacBridgeContract::finder_required();

        assert!(contract
            .bridges
            .iter()
            .any(|bridge| bridge.id == "foundation-host-profile"
                && bridge.status == MacBridgeStatus::Implemented));
        assert!(contract
            .bridges
            .iter()
            .any(|bridge| bridge.id == "launchservices-icons-and-packages"
                && bridge.status == MacBridgeStatus::Required));
        assert_eq!(
            contract.implemented_count() + contract.required_count(),
            contract.bridges.len()
        );
    }

    #[test]
    fn bridge_contract_tsv_is_stable_for_cli_and_fozzy() {
        let contract = MacBridgeContract::finder_required();
        let tsv = contract.as_tsv();

        assert!(tsv.starts_with("mac-bridges\timplemented=7\trequired=5\ttotal=12"));
        assert!(tsv.contains(
            "bridge\tappkit-window-shell\tappkit\tcrates/ui\tapplication-window-menu-activation\tmain-thread\timplemented"
        ));
        assert!(tsv.contains(
            "bridge\tcoreservices-kind-string\tcoreservices\tcrates/mac\tlaunchservices-localized-kind-string\tbackground-safe\timplemented"
        ));
        assert!(tsv.contains(
            "bridge\tspotlight-metadata-reconciliation\tspotlight\tcrates/index\tmetadata-import-without-primary-correctness-dependency\tbackground-safe\timplemented"
        ));
        assert!(tsv.contains(
            "bridge\tcorefoundation-security-bookmarks\tsecurity\tcrates/mac-sys\tsecurity-scoped-bookmark-create-resolve-access\tbackground-safe\timplemented"
        ));
    }
}
