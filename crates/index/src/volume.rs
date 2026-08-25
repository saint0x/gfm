use gfm_config::VolumeIndexingPolicy;
use gfm_types::{GfmError, Result};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexVolumeClass {
    System,
    Internal,
    External,
    Network,
    Unknown,
}

impl IndexVolumeClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Internal => "internal",
            Self::External => "external",
            Self::Network => "network",
            Self::Unknown => "unknown",
        }
    }

    const fn policy_family(self) -> VolumePolicyFamily {
        match self {
            Self::External => VolumePolicyFamily::External,
            Self::Network => VolumePolicyFamily::Network,
            Self::System | Self::Internal | Self::Unknown => VolumePolicyFamily::Local,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexMountState {
    Mounted,
    Unmounted,
    Stale,
}

impl IndexMountState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mounted => "mounted",
            Self::Unmounted => "unmounted",
            Self::Stale => "stale",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeIndexAction {
    Include,
    DeferredOptIn,
    Disabled,
    Disconnected,
}

impl VolumeIndexAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Include => "include",
            Self::DeferredOptIn => "deferred-opt-in",
            Self::Disabled => "disabled",
            Self::Disconnected => "disconnected",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeThrottleClass {
    Local,
    External,
    Network,
    Suspended,
}

impl VolumeThrottleClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::External => "external",
            Self::Network => "network",
            Self::Suspended => "suspended",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeIndexThrottle {
    pub class: VolumeThrottleClass,
    pub max_concurrent_jobs: usize,
    pub crawl_delay: Duration,
    pub content_bytes_per_second: Option<u64>,
}

impl VolumeIndexThrottle {
    pub const fn local() -> Self {
        Self {
            class: VolumeThrottleClass::Local,
            max_concurrent_jobs: 4,
            crawl_delay: Duration::from_millis(0),
            content_bytes_per_second: None,
        }
    }

    pub const fn external() -> Self {
        Self {
            class: VolumeThrottleClass::External,
            max_concurrent_jobs: 2,
            crawl_delay: Duration::from_millis(2),
            content_bytes_per_second: Some(96 * 1024 * 1024),
        }
    }

    pub const fn network() -> Self {
        Self {
            class: VolumeThrottleClass::Network,
            max_concurrent_jobs: 1,
            crawl_delay: Duration::from_millis(25),
            content_bytes_per_second: Some(16 * 1024 * 1024),
        }
    }

    pub const fn suspended() -> Self {
        Self {
            class: VolumeThrottleClass::Suspended,
            max_concurrent_jobs: 0,
            crawl_delay: Duration::from_millis(0),
            content_bytes_per_second: Some(0),
        }
    }

    fn as_tsv_fields(&self) -> String {
        format!(
            "throttle={}\tmax-jobs={}\tcrawl-delay-ms={}\tcontent-bps={}",
            self.class.as_str(),
            self.max_concurrent_jobs,
            self.crawl_delay.as_millis(),
            self.content_bytes_per_second
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unbounded".to_string())
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexVolumeDescriptor {
    pub label: String,
    pub path: PathBuf,
    pub class: IndexVolumeClass,
    pub mount_state: IndexMountState,
}

impl IndexVolumeDescriptor {
    pub fn new(
        label: impl Into<String>,
        path: impl Into<PathBuf>,
        class: IndexVolumeClass,
        mount_state: IndexMountState,
    ) -> Self {
        Self {
            label: label.into(),
            path: path.into(),
            class,
            mount_state,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeIndexPolicy {
    pub external: VolumeIndexingPolicy,
    pub network: VolumeIndexingPolicy,
    pub opted_in_roots: Vec<PathBuf>,
}

impl Default for VolumeIndexPolicy {
    fn default() -> Self {
        Self {
            external: VolumeIndexingPolicy::OptIn,
            network: VolumeIndexingPolicy::OptIn,
            opted_in_roots: Vec::new(),
        }
    }
}

impl VolumeIndexPolicy {
    pub fn new(external: VolumeIndexingPolicy, network: VolumeIndexingPolicy) -> Self {
        Self {
            external,
            network,
            opted_in_roots: Vec::new(),
        }
    }

    pub fn with_opted_in_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.opted_in_roots = roots;
        self
    }

    pub fn decide(&self, volume: &IndexVolumeDescriptor) -> VolumeIndexDecision {
        if volume.mount_state != IndexMountState::Mounted {
            return VolumeIndexDecision::new(
                volume,
                VolumeIndexAction::Disconnected,
                VolumeIndexThrottle::suspended(),
                "volume-disconnected",
            );
        }

        match volume.class.policy_family() {
            VolumePolicyFamily::Local => VolumeIndexDecision::new(
                volume,
                VolumeIndexAction::Include,
                VolumeIndexThrottle::local(),
                "local-volume",
            ),
            VolumePolicyFamily::External => {
                self.decide_remote(volume, self.external, VolumeIndexThrottle::external())
            }
            VolumePolicyFamily::Network => {
                self.decide_remote(volume, self.network, VolumeIndexThrottle::network())
            }
        }
    }

    pub fn plan(&self, volumes: Vec<IndexVolumeDescriptor>) -> VolumeIndexPlan {
        let mut decisions = volumes
            .iter()
            .map(|volume| self.decide(volume))
            .collect::<Vec<_>>();
        decisions.sort_by(|left, right| left.path.cmp(&right.path));
        VolumeIndexPlan { decisions }
    }

    fn decide_remote(
        &self,
        volume: &IndexVolumeDescriptor,
        policy: VolumeIndexingPolicy,
        throttle: VolumeIndexThrottle,
    ) -> VolumeIndexDecision {
        match policy {
            VolumeIndexingPolicy::Disabled => VolumeIndexDecision::new(
                volume,
                VolumeIndexAction::Disabled,
                VolumeIndexThrottle::suspended(),
                "policy-disabled",
            ),
            VolumeIndexingPolicy::Enabled => VolumeIndexDecision::new(
                volume,
                VolumeIndexAction::Include,
                throttle,
                "policy-enabled",
            ),
            VolumeIndexingPolicy::OptIn if self.is_opted_in(&volume.path) => {
                VolumeIndexDecision::new(volume, VolumeIndexAction::Include, throttle, "opted-in")
            }
            VolumeIndexingPolicy::OptIn => VolumeIndexDecision::new(
                volume,
                VolumeIndexAction::DeferredOptIn,
                VolumeIndexThrottle::suspended(),
                "requires-opt-in",
            ),
        }
    }

    fn is_opted_in(&self, path: &Path) -> bool {
        self.opted_in_roots
            .iter()
            .any(|root| path_is_same_or_child(path, root))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeIndexDecision {
    pub label: String,
    pub path: PathBuf,
    pub class: IndexVolumeClass,
    pub mount_state: IndexMountState,
    pub action: VolumeIndexAction,
    pub throttle: VolumeIndexThrottle,
    pub reason: String,
}

impl VolumeIndexDecision {
    fn new(
        volume: &IndexVolumeDescriptor,
        action: VolumeIndexAction,
        throttle: VolumeIndexThrottle,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            label: volume.label.clone(),
            path: volume.path.clone(),
            class: volume.class,
            mount_state: volume.mount_state,
            action,
            throttle,
            reason: reason.into(),
        }
    }

    pub fn should_index(&self) -> bool {
        self.action == VolumeIndexAction::Include
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "volume-index\t{}\tpath={}\tclass={}\tmount={}\taction={}\t{}\treason={}",
            escape_field(&self.label),
            self.path.display(),
            self.class.as_str(),
            self.mount_state.as_str(),
            self.action.as_str(),
            self.throttle.as_tsv_fields(),
            escape_field(&self.reason)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeIndexPlan {
    pub decisions: Vec<VolumeIndexDecision>,
}

impl VolumeIndexPlan {
    pub fn included_roots(&self) -> Vec<PathBuf> {
        self.decisions
            .iter()
            .filter(|decision| decision.should_index())
            .map(|decision| decision.path.clone())
            .collect()
    }

    pub fn as_tsv(&self) -> String {
        let included = self
            .decisions
            .iter()
            .filter(|decision| decision.should_index())
            .count();
        let mut lines = vec![format!(
            "volume-index-plan\tcount={}\tincluded={}",
            self.decisions.len(),
            included
        )];
        lines.extend(self.decisions.iter().map(VolumeIndexDecision::as_tsv));
        lines.join("\n")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VolumePolicyFamily {
    Local,
    External,
    Network,
}

pub fn parse_volume_indexing_policy(input: &str) -> Result<VolumeIndexingPolicy> {
    match input.trim().to_ascii_lowercase().as_str() {
        "disabled" => Ok(VolumeIndexingPolicy::Disabled),
        "opt-in" | "optin" => Ok(VolumeIndexingPolicy::OptIn),
        "enabled" => Ok(VolumeIndexingPolicy::Enabled),
        _ => Err(GfmError::Format(format!(
            "unsupported volume indexing policy `{input}`"
        ))),
    }
}

pub const fn volume_indexing_policy_name(policy: VolumeIndexingPolicy) -> &'static str {
    match policy {
        VolumeIndexingPolicy::Disabled => "disabled",
        VolumeIndexingPolicy::OptIn => "opt-in",
        VolumeIndexingPolicy::Enabled => "enabled",
    }
}

fn path_is_same_or_child(path: &Path, root: &Path) -> bool {
    if path == root {
        return true;
    }
    path.starts_with(root)
}

fn escape_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}
