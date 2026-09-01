use gfm_config::VolumeIndexingPolicy;
use gfm_search::SearchVolumeScope;
use gfm_types::{GfmError, Result, VolumeId};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexVolumeClass {
    System,
    Internal,
    External,
    Slow,
    Network,
    Unknown,
}

impl IndexVolumeClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Internal => "internal",
            Self::External => "external",
            Self::Slow => "slow",
            Self::Network => "network",
            Self::Unknown => "unknown",
        }
    }

    const fn policy_family(self) -> VolumePolicyFamily {
        match self {
            Self::External | Self::Slow => VolumePolicyFamily::External,
            Self::Network => VolumePolicyFamily::Network,
            Self::Unknown => VolumePolicyFamily::Network,
            Self::System | Self::Internal => VolumePolicyFamily::Local,
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "system" => Ok(Self::System),
            "internal" => Ok(Self::Internal),
            "external" | "removable" => Ok(Self::External),
            "slow" | "disk-image" => Ok(Self::Slow),
            "network" => Ok(Self::Network),
            "unknown" => Ok(Self::Unknown),
            other => Err(GfmError::Format(format!(
                "unsupported index volume class `{other}`"
            ))),
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

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "mounted" => Ok(Self::Mounted),
            "unmounted" => Ok(Self::Unmounted),
            "stale" | "disconnected" => Ok(Self::Stale),
            other => Err(GfmError::Format(format!(
                "unsupported index mount state `{other}`"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeIndexAction {
    Include,
    DeferredOptIn,
    Disabled,
    Disconnected,
    Unreachable,
}

impl VolumeIndexAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Include => "include",
            Self::DeferredOptIn => "deferred-opt-in",
            Self::Disabled => "disabled",
            Self::Disconnected => "disconnected",
            Self::Unreachable => "unreachable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexVolumeEventKind {
    Appeared,
    DescriptionChanged,
    Disappeared,
    Unavailable,
}

impl IndexVolumeEventKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Appeared => "appeared",
            Self::DescriptionChanged => "description-changed",
            Self::Disappeared => "disappeared",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeThrottleClass {
    Local,
    External,
    Slow,
    Network,
    Suspended,
}

impl VolumeThrottleClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::External => "external",
            Self::Slow => "slow",
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

    pub const fn slow() -> Self {
        Self {
            class: VolumeThrottleClass::Slow,
            max_concurrent_jobs: 1,
            crawl_delay: Duration::from_millis(8),
            content_bytes_per_second: Some(32 * 1024 * 1024),
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
    pub id: Option<VolumeId>,
    pub label: String,
    pub path: PathBuf,
    pub class: IndexVolumeClass,
    pub mount_state: IndexMountState,
    pub reachable: Option<bool>,
    pub read_only: Option<bool>,
    pub writable: Option<bool>,
    pub ejectable: Option<bool>,
    pub mountable: Option<bool>,
    pub case_sensitive: Option<bool>,
    pub stable_identity: Option<String>,
    pub filesystem_signature: Option<String>,
    pub native_status: Option<String>,
    pub native_reason: Option<String>,
    pub resource_status: Option<String>,
    pub resource_reason: Option<String>,
    pub mount_status: Option<String>,
    pub mount_reason: Option<String>,
}

impl IndexVolumeDescriptor {
    pub fn new(
        label: impl Into<String>,
        path: impl Into<PathBuf>,
        class: IndexVolumeClass,
        mount_state: IndexMountState,
    ) -> Self {
        Self {
            id: None,
            label: label.into(),
            path: path.into(),
            class,
            mount_state,
            reachable: Some(mount_state == IndexMountState::Mounted),
            read_only: None,
            writable: None,
            ejectable: None,
            mountable: None,
            case_sensitive: None,
            stable_identity: None,
            filesystem_signature: None,
            native_status: None,
            native_reason: None,
            resource_status: None,
            resource_reason: None,
            mount_status: None,
            mount_reason: None,
        }
    }

    pub fn with_volume_id(mut self, id: VolumeId) -> Self {
        self.id = Some(id);
        self
    }

    pub fn with_stable_identity(mut self, stable_identity: impl Into<String>) -> Self {
        self.stable_identity = Some(stable_identity.into());
        self
    }

    pub fn with_reachable(mut self, reachable: Option<bool>) -> Self {
        self.reachable = reachable;
        self
    }

    pub fn with_read_only(mut self, read_only: Option<bool>) -> Self {
        self.read_only = read_only;
        self
    }

    pub fn with_writable(mut self, writable: Option<bool>) -> Self {
        self.writable = writable;
        self
    }

    pub fn with_ejectable(mut self, ejectable: Option<bool>) -> Self {
        self.ejectable = ejectable;
        self
    }

    pub fn with_mountable(mut self, mountable: Option<bool>) -> Self {
        self.mountable = mountable;
        self
    }

    pub fn with_case_sensitive(mut self, case_sensitive: Option<bool>) -> Self {
        self.case_sensitive = case_sensitive;
        self
    }

    pub fn with_filesystem_signature(mut self, filesystem_signature: impl Into<String>) -> Self {
        self.filesystem_signature = Some(filesystem_signature.into());
        self
    }

    pub fn with_native_status(mut self, status: impl Into<String>) -> Self {
        self.native_status = normalized_descriptor_field(status);
        self
    }

    pub fn with_native_reason(mut self, reason: impl Into<String>) -> Self {
        self.native_reason = normalized_descriptor_field(reason);
        self
    }

    pub fn with_resource_status(mut self, status: impl Into<String>) -> Self {
        self.resource_status = normalized_descriptor_field(status);
        self
    }

    pub fn with_resource_reason(mut self, reason: impl Into<String>) -> Self {
        self.resource_reason = normalized_descriptor_field(reason);
        self
    }

    pub fn with_mount_status(mut self, status: impl Into<String>) -> Self {
        self.mount_status = normalized_descriptor_field(status);
        self
    }

    pub fn with_mount_reason(mut self, reason: impl Into<String>) -> Self {
        self.mount_reason = normalized_descriptor_field(reason);
        self
    }
}

fn normalized_descriptor_field(value: impl Into<String>) -> Option<String> {
    let value = value.into();
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
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
        if volume.reachable == Some(false) {
            return VolumeIndexDecision::new(
                volume,
                VolumeIndexAction::Unreachable,
                VolumeIndexThrottle::suspended(),
                "volume-unreachable",
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
                let throttle = match volume.class {
                    IndexVolumeClass::Slow => VolumeIndexThrottle::slow(),
                    _ => VolumeIndexThrottle::external(),
                };
                self.decide_remote(volume, self.external, throttle)
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
    pub id: Option<VolumeId>,
    pub label: String,
    pub path: PathBuf,
    pub class: IndexVolumeClass,
    pub mount_state: IndexMountState,
    pub reachable: Option<bool>,
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
            id: volume.id,
            label: volume.label.clone(),
            path: volume.path.clone(),
            class: volume.class,
            mount_state: volume.mount_state,
            reachable: volume.reachable,
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
            "volume-index\t{}\tid={}\tpath={}\tclass={}\tmount={}\treachable={}\taction={}\t{}\treason={}",
            escape_field(&self.label),
            self.id
                .map(|id| id.0.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.path.display(),
            self.class.as_str(),
            self.mount_state.as_str(),
            self.reachable
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeInvalidationReport {
    pub path: PathBuf,
    pub previous_class: Option<IndexVolumeClass>,
    pub previous_mount_state: Option<IndexMountState>,
    pub previous_reachable: Option<bool>,
    pub previous_read_only: Option<bool>,
    pub previous_writable: Option<bool>,
    pub previous_ejectable: Option<bool>,
    pub previous_mountable: Option<bool>,
    pub previous_case_sensitive: Option<bool>,
    pub current_class: Option<IndexVolumeClass>,
    pub current_mount_state: Option<IndexMountState>,
    pub current_reachable: Option<bool>,
    pub current_read_only: Option<bool>,
    pub current_writable: Option<bool>,
    pub current_ejectable: Option<bool>,
    pub current_mountable: Option<bool>,
    pub current_case_sensitive: Option<bool>,
    pub invalidate_sidebar: bool,
    pub invalidate_operation_policy: bool,
    pub invalidate_index_admission: bool,
    pub rescan_index: bool,
    pub cancel_index_jobs: bool,
    pub clear_fsevents_cursor: bool,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeEventIndexInvalidationReport {
    pub kind: IndexVolumeEventKind,
    pub path: Option<PathBuf>,
    pub previous_volume_id: Option<VolumeId>,
    pub previous_class: Option<IndexVolumeClass>,
    pub previous_mount_state: Option<IndexMountState>,
    pub previous_reachable: Option<bool>,
    pub previous_read_only: Option<bool>,
    pub previous_writable: Option<bool>,
    pub previous_ejectable: Option<bool>,
    pub previous_mountable: Option<bool>,
    pub previous_case_sensitive: Option<bool>,
    pub previous_native_status: Option<String>,
    pub previous_native_reason: Option<String>,
    pub previous_resource_status: Option<String>,
    pub previous_resource_reason: Option<String>,
    pub previous_mount_status: Option<String>,
    pub previous_mount_reason: Option<String>,
    pub current_volume_id: Option<VolumeId>,
    pub current_class: Option<IndexVolumeClass>,
    pub current_mount_state: Option<IndexMountState>,
    pub current_reachable: Option<bool>,
    pub current_read_only: Option<bool>,
    pub current_writable: Option<bool>,
    pub current_ejectable: Option<bool>,
    pub current_mountable: Option<bool>,
    pub current_case_sensitive: Option<bool>,
    pub current_native_status: Option<String>,
    pub current_native_reason: Option<String>,
    pub current_resource_status: Option<String>,
    pub current_resource_reason: Option<String>,
    pub current_mount_status: Option<String>,
    pub current_mount_reason: Option<String>,
    pub read_only_changed: bool,
    pub writable_changed: bool,
    pub ejectable_changed: bool,
    pub mountable_changed: bool,
    pub case_sensitive_changed: bool,
    pub api_status_changed: bool,
    pub stable_identity_changed: bool,
    pub filesystem_signature_changed: bool,
    pub invalidate_index_admission: bool,
    pub rescan_index: bool,
    pub cancel_index_jobs: bool,
    pub clear_fsevents_cursor: bool,
    pub reason: String,
}

impl VolumeInvalidationReport {
    pub fn evaluate(
        previous: Option<&IndexVolumeDescriptor>,
        current: Option<&IndexVolumeDescriptor>,
    ) -> Self {
        let path = current
            .or(previous)
            .map(|volume| volume.path.clone())
            .unwrap_or_else(|| PathBuf::from("-"));
        let previous_class = previous.map(|volume| volume.class);
        let previous_mount_state = previous.map(|volume| volume.mount_state);
        let previous_reachable = previous.and_then(|volume| volume.reachable);
        let previous_read_only = previous.and_then(|volume| volume.read_only);
        let previous_writable = previous.and_then(|volume| volume.writable);
        let previous_ejectable = previous.and_then(|volume| volume.ejectable);
        let previous_mountable = previous.and_then(|volume| volume.mountable);
        let previous_case_sensitive = previous.and_then(|volume| volume.case_sensitive);
        let current_class = current.map(|volume| volume.class);
        let current_mount_state = current.map(|volume| volume.mount_state);
        let current_reachable = current.and_then(|volume| volume.reachable);
        let current_read_only = current.and_then(|volume| volume.read_only);
        let current_writable = current.and_then(|volume| volume.writable);
        let current_ejectable = current.and_then(|volume| volume.ejectable);
        let current_mountable = current.and_then(|volume| volume.mountable);
        let current_case_sensitive = current.and_then(|volume| volume.case_sensitive);

        let (
            invalidate_sidebar,
            invalidate_operation_policy,
            invalidate_index_admission,
            rescan_index,
            cancel_index_jobs,
            clear_fsevents_cursor,
            reason,
        ) = match (previous, current) {
            (None, None) => (false, false, false, false, false, false, "unchanged"),
            (None, Some(_)) => (true, true, true, true, false, true, "volume-connected"),
            (Some(previous), None) => (
                true,
                true,
                true,
                true,
                previous.mount_state == IndexMountState::Mounted,
                true,
                "volume-disconnected",
            ),
            (Some(previous), Some(current)) if previous.path != current.path => {
                (true, true, true, true, true, true, "volume-path-changed")
            }
            (Some(previous), Some(current)) if previous.mount_state != current.mount_state => (
                true,
                true,
                true,
                true,
                previous.mount_state == IndexMountState::Mounted,
                true,
                "mount-state-changed",
            ),
            (Some(previous), Some(current)) if previous.class != current.class => {
                (true, true, true, true, true, true, "volume-class-changed")
            }
            (Some(previous), Some(current))
                if known_optional_value_lost_or_changed(
                    &previous.reachable,
                    &current.reachable,
                ) =>
            {
                (
                    true,
                    true,
                    true,
                    true,
                    previous.mount_state == IndexMountState::Mounted,
                    true,
                    "volume-reachability-changed",
                )
            }
            (Some(previous), Some(current))
                if known_optional_value_lost_or_changed(
                    &previous.read_only,
                    &current.read_only,
                ) =>
            {
                (
                    true,
                    true,
                    true,
                    true,
                    previous.mount_state == IndexMountState::Mounted,
                    true,
                    "volume-read-only-changed",
                )
            }
            (Some(previous), Some(current))
                if known_optional_value_lost_or_changed(&previous.writable, &current.writable) =>
            {
                (
                    true,
                    true,
                    true,
                    true,
                    previous.mount_state == IndexMountState::Mounted,
                    true,
                    "volume-writable-changed",
                )
            }
            (Some(previous), Some(current))
                if known_optional_value_lost_or_changed(
                    &previous.ejectable,
                    &current.ejectable,
                ) =>
            {
                (
                    true,
                    true,
                    true,
                    true,
                    previous.mount_state == IndexMountState::Mounted,
                    true,
                    "volume-ejectable-changed",
                )
            }
            (Some(previous), Some(current))
                if known_optional_value_lost_or_changed(
                    &previous.mountable,
                    &current.mountable,
                ) =>
            {
                (
                    true,
                    true,
                    true,
                    true,
                    previous.mount_state == IndexMountState::Mounted,
                    true,
                    "volume-mountable-changed",
                )
            }
            (Some(previous), Some(current))
                if known_optional_value_lost_or_changed(
                    &previous.case_sensitive,
                    &current.case_sensitive,
                ) =>
            {
                (
                    true,
                    true,
                    true,
                    true,
                    previous.mount_state == IndexMountState::Mounted,
                    true,
                    "volume-case-sensitivity-changed",
                )
            }
            (Some(previous), Some(current))
                if known_optional_value_lost_or_changed(
                    &previous.stable_identity,
                    &current.stable_identity,
                ) =>
            {
                (
                    true,
                    true,
                    true,
                    true,
                    previous.mount_state == IndexMountState::Mounted,
                    true,
                    "volume-identity-changed",
                )
            }
            (Some(previous), Some(current))
                if known_optional_value_lost_or_changed(
                    &previous.filesystem_signature,
                    &current.filesystem_signature,
                ) =>
            {
                (
                    true,
                    true,
                    true,
                    true,
                    previous.mount_state == IndexMountState::Mounted,
                    true,
                    "volume-filesystem-changed",
                )
            }
            (Some(previous), Some(current)) if previous.label != current.label => (
                true,
                false,
                false,
                false,
                false,
                false,
                "volume-label-changed",
            ),
            (Some(_), Some(_)) => (false, false, false, false, false, false, "unchanged"),
        };

        Self {
            path,
            previous_class,
            previous_mount_state,
            previous_reachable,
            previous_read_only,
            previous_writable,
            previous_ejectable,
            previous_mountable,
            previous_case_sensitive,
            current_class,
            current_mount_state,
            current_reachable,
            current_read_only,
            current_writable,
            current_ejectable,
            current_mountable,
            current_case_sensitive,
            invalidate_sidebar,
            invalidate_operation_policy,
            invalidate_index_admission,
            rescan_index,
            cancel_index_jobs,
            clear_fsevents_cursor,
            reason: reason.to_string(),
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "volume-invalidation\tpath={}\tprevious-class={}\tprevious-mount={}\tprevious-reachable={}\tprevious-read-only={}\tcurrent-class={}\tcurrent-mount={}\tcurrent-reachable={}\tcurrent-read-only={}\tsidebar={}\toperation-policy={}\tindex-admission={}\trescan-index={}\tcancel-index-jobs={}\tclear-fsevents-cursor={}\tprevious-writable={}\tprevious-ejectable={}\tprevious-mountable={}\tprevious-case-sensitive={}\tcurrent-writable={}\tcurrent-ejectable={}\tcurrent-mountable={}\tcurrent-case-sensitive={}\treason={}",
            self.path.display(),
            self.previous_class
                .map(IndexVolumeClass::as_str)
                .unwrap_or("-"),
            self.previous_mount_state
                .map(IndexMountState::as_str)
                .unwrap_or("-"),
            self.previous_reachable
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.previous_read_only
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.current_class.map(IndexVolumeClass::as_str).unwrap_or("-"),
            self.current_mount_state
                .map(IndexMountState::as_str)
                .unwrap_or("-"),
            self.current_reachable
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.current_read_only
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.invalidate_sidebar,
            self.invalidate_operation_policy,
            self.invalidate_index_admission,
            self.rescan_index,
            self.cancel_index_jobs,
            self.clear_fsevents_cursor,
            format_optional_bool(self.previous_writable),
            format_optional_bool(self.previous_ejectable),
            format_optional_bool(self.previous_mountable),
            format_optional_bool(self.previous_case_sensitive),
            format_optional_bool(self.current_writable),
            format_optional_bool(self.current_ejectable),
            format_optional_bool(self.current_mountable),
            format_optional_bool(self.current_case_sensitive),
            escape_field(&self.reason)
        )
    }
}

impl VolumeEventIndexInvalidationReport {
    pub fn from_event(
        kind: IndexVolumeEventKind,
        path: Option<PathBuf>,
        previous: Option<&IndexVolumeDescriptor>,
        current: Option<&IndexVolumeDescriptor>,
        source_invalidates_index_admission: bool,
        source_rescans_index: bool,
    ) -> Self {
        let stable_identity_changed = known_optional_value_lost_or_changed(
            &previous.and_then(|volume| volume.stable_identity.clone()),
            &current.and_then(|volume| volume.stable_identity.clone()),
        );
        let filesystem_signature_changed = known_optional_value_lost_or_changed(
            &previous.and_then(|volume| volume.filesystem_signature.clone()),
            &current.and_then(|volume| volume.filesystem_signature.clone()),
        );
        let read_only_changed = known_optional_value_lost_or_changed(
            &previous.and_then(|volume| volume.read_only),
            &current.and_then(|volume| volume.read_only),
        );
        let writable_changed = known_optional_value_lost_or_changed(
            &previous.and_then(|volume| volume.writable),
            &current.and_then(|volume| volume.writable),
        );
        let ejectable_changed = known_optional_value_lost_or_changed(
            &previous.and_then(|volume| volume.ejectable),
            &current.and_then(|volume| volume.ejectable),
        );
        let mountable_changed = known_optional_value_lost_or_changed(
            &previous.and_then(|volume| volume.mountable),
            &current.and_then(|volume| volume.mountable),
        );
        let case_sensitive_changed = known_optional_value_lost_or_changed(
            &previous.and_then(|volume| volume.case_sensitive),
            &current.and_then(|volume| volume.case_sensitive),
        );
        let native_status_changed = known_optional_value_lost_or_changed(
            &previous.and_then(|volume| volume.native_status.clone()),
            &current.and_then(|volume| volume.native_status.clone()),
        );
        let resource_status_changed = known_optional_value_lost_or_changed(
            &previous.and_then(|volume| volume.resource_status.clone()),
            &current.and_then(|volume| volume.resource_status.clone()),
        );
        let mount_status_changed = known_optional_value_lost_or_changed(
            &previous.and_then(|volume| volume.mount_status.clone()),
            &current.and_then(|volume| volume.mount_status.clone()),
        );
        let api_status_changed =
            native_status_changed || resource_status_changed || mount_status_changed;
        let event_visible = path.is_some()
            || previous.is_some()
            || current.is_some()
            || source_invalidates_index_admission;
        let descriptor_changed = stable_identity_changed
            || filesystem_signature_changed
            || read_only_changed
            || writable_changed
            || ejectable_changed
            || mountable_changed
            || case_sensitive_changed
            || api_status_changed;
        let invalidate_index_admission =
            event_visible && (source_invalidates_index_admission || descriptor_changed);
        let rescan_index = event_visible && (source_rescans_index || descriptor_changed);
        let cancel_index_jobs = event_visible
            && match kind {
                IndexVolumeEventKind::Appeared => false,
                IndexVolumeEventKind::DescriptionChanged => {
                    descriptor_changed || source_invalidates_index_admission || source_rescans_index
                }
                IndexVolumeEventKind::Disappeared | IndexVolumeEventKind::Unavailable => true,
            };
        let clear_fsevents_cursor = invalidate_index_admission || rescan_index || cancel_index_jobs;
        let reason = match kind {
            IndexVolumeEventKind::Appeared if current.is_some() => "volume-event-connected",
            IndexVolumeEventKind::Appeared => "volume-event-appeared-unclassified",
            IndexVolumeEventKind::DescriptionChanged if stable_identity_changed => {
                "volume-event-identity-changed"
            }
            IndexVolumeEventKind::DescriptionChanged if read_only_changed => {
                "volume-event-read-only-changed"
            }
            IndexVolumeEventKind::DescriptionChanged if writable_changed => {
                "volume-event-writable-changed"
            }
            IndexVolumeEventKind::DescriptionChanged if ejectable_changed => {
                "volume-event-ejectable-changed"
            }
            IndexVolumeEventKind::DescriptionChanged if mountable_changed => {
                "volume-event-mountable-changed"
            }
            IndexVolumeEventKind::DescriptionChanged if case_sensitive_changed => {
                "volume-event-case-sensitivity-changed"
            }
            IndexVolumeEventKind::DescriptionChanged if api_status_changed => {
                "volume-event-api-status-changed"
            }
            IndexVolumeEventKind::DescriptionChanged if filesystem_signature_changed => {
                "volume-event-filesystem-changed"
            }
            IndexVolumeEventKind::DescriptionChanged
                if current.is_some()
                    && (source_invalidates_index_admission
                        || source_rescans_index
                        || descriptor_changed) =>
            {
                "volume-event-descriptor-changed"
            }
            IndexVolumeEventKind::DescriptionChanged if current.is_some() => {
                "volume-event-description-sidebar-only"
            }
            IndexVolumeEventKind::DescriptionChanged => "volume-event-description-unavailable",
            IndexVolumeEventKind::Disappeared => "volume-event-disconnected",
            IndexVolumeEventKind::Unavailable => "volume-event-native-unavailable",
        };

        Self {
            kind,
            path,
            previous_volume_id: previous.and_then(|volume| volume.id),
            previous_class: previous.map(|volume| volume.class),
            previous_mount_state: previous.map(|volume| volume.mount_state),
            previous_reachable: previous.and_then(|volume| volume.reachable),
            previous_read_only: previous.and_then(|volume| volume.read_only),
            previous_writable: previous.and_then(|volume| volume.writable),
            previous_ejectable: previous.and_then(|volume| volume.ejectable),
            previous_mountable: previous.and_then(|volume| volume.mountable),
            previous_case_sensitive: previous.and_then(|volume| volume.case_sensitive),
            previous_native_status: previous.and_then(|volume| volume.native_status.clone()),
            previous_native_reason: previous.and_then(|volume| volume.native_reason.clone()),
            previous_resource_status: previous.and_then(|volume| volume.resource_status.clone()),
            previous_resource_reason: previous.and_then(|volume| volume.resource_reason.clone()),
            previous_mount_status: previous.and_then(|volume| volume.mount_status.clone()),
            previous_mount_reason: previous.and_then(|volume| volume.mount_reason.clone()),
            current_volume_id: current.and_then(|volume| volume.id),
            current_class: current.map(|volume| volume.class),
            current_mount_state: current.map(|volume| volume.mount_state),
            current_reachable: current.and_then(|volume| volume.reachable),
            current_read_only: current.and_then(|volume| volume.read_only),
            current_writable: current.and_then(|volume| volume.writable),
            current_ejectable: current.and_then(|volume| volume.ejectable),
            current_mountable: current.and_then(|volume| volume.mountable),
            current_case_sensitive: current.and_then(|volume| volume.case_sensitive),
            current_native_status: current.and_then(|volume| volume.native_status.clone()),
            current_native_reason: current.and_then(|volume| volume.native_reason.clone()),
            current_resource_status: current.and_then(|volume| volume.resource_status.clone()),
            current_resource_reason: current.and_then(|volume| volume.resource_reason.clone()),
            current_mount_status: current.and_then(|volume| volume.mount_status.clone()),
            current_mount_reason: current.and_then(|volume| volume.mount_reason.clone()),
            read_only_changed,
            writable_changed,
            ejectable_changed,
            mountable_changed,
            case_sensitive_changed,
            api_status_changed,
            stable_identity_changed,
            filesystem_signature_changed,
            invalidate_index_admission,
            rescan_index,
            cancel_index_jobs,
            clear_fsevents_cursor,
            reason: reason.to_string(),
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "volume-event-index-invalidation\tkind={}\tpath={}\tprevious-volume={}\tprevious-class={}\tprevious-mount={}\tprevious-reachable={}\tprevious-read-only={}\tcurrent-volume={}\tcurrent-class={}\tcurrent-mount={}\tcurrent-reachable={}\tcurrent-read-only={}\tread-only-changed={}\tidentity-changed={}\tfilesystem-changed={}\tindex-admission={}\trescan-index={}\tcancel-index-jobs={}\tclear-fsevents-cursor={}\tprevious-writable={}\tprevious-ejectable={}\tprevious-mountable={}\tprevious-case-sensitive={}\tcurrent-writable={}\tcurrent-ejectable={}\tcurrent-mountable={}\tcurrent-case-sensitive={}\twritable-changed={}\tejectable-changed={}\tmountable-changed={}\tcase-sensitive-changed={}\tprevious-native-status={}\tprevious-native-reason={}\tprevious-resource-status={}\tprevious-resource-reason={}\tprevious-mount-status={}\tprevious-mount-reason={}\tcurrent-native-status={}\tcurrent-native-reason={}\tcurrent-resource-status={}\tcurrent-resource-reason={}\tcurrent-mount-status={}\tcurrent-mount-reason={}\tapi-status-changed={}\treason={}",
            self.kind.as_str(),
            self.path
                .as_ref()
                .map(|path| escape_field(&path.to_string_lossy()))
                .unwrap_or_else(|| "-".to_string()),
            self.previous_volume_id
                .map(|id| id.0.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.previous_class
                .map(IndexVolumeClass::as_str)
                .unwrap_or("-"),
            self.previous_mount_state
                .map(IndexMountState::as_str)
                .unwrap_or("-"),
            self.previous_reachable
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.previous_read_only
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.current_volume_id
                .map(|id| id.0.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.current_class.map(IndexVolumeClass::as_str).unwrap_or("-"),
            self.current_mount_state
                .map(IndexMountState::as_str)
                .unwrap_or("-"),
            self.current_reachable
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.current_read_only
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.read_only_changed,
            self.stable_identity_changed,
            self.filesystem_signature_changed,
            self.invalidate_index_admission,
            self.rescan_index,
            self.cancel_index_jobs,
            self.clear_fsevents_cursor,
            format_optional_bool(self.previous_writable),
            format_optional_bool(self.previous_ejectable),
            format_optional_bool(self.previous_mountable),
            format_optional_bool(self.previous_case_sensitive),
            format_optional_bool(self.current_writable),
            format_optional_bool(self.current_ejectable),
            format_optional_bool(self.current_mountable),
            format_optional_bool(self.current_case_sensitive),
            self.writable_changed,
            self.ejectable_changed,
            self.mountable_changed,
            self.case_sensitive_changed,
            format_optional_string(self.previous_native_status.as_deref()),
            format_optional_string(self.previous_native_reason.as_deref()),
            format_optional_string(self.previous_resource_status.as_deref()),
            format_optional_string(self.previous_resource_reason.as_deref()),
            format_optional_string(self.previous_mount_status.as_deref()),
            format_optional_string(self.previous_mount_reason.as_deref()),
            format_optional_string(self.current_native_status.as_deref()),
            format_optional_string(self.current_native_reason.as_deref()),
            format_optional_string(self.current_resource_status.as_deref()),
            format_optional_string(self.current_resource_reason.as_deref()),
            format_optional_string(self.current_mount_status.as_deref()),
            format_optional_string(self.current_mount_reason.as_deref()),
            self.api_status_changed,
            escape_field(&self.reason)
        )
    }
}

fn known_optional_value_lost_or_changed<T: Eq>(previous: &Option<T>, current: &Option<T>) -> bool {
    matches!((previous, current), (Some(_), None) | (Some(_), Some(_))) && previous != current
}

fn format_optional_bool(value: Option<bool>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn format_optional_string(value: Option<&str>) -> String {
    value
        .filter(|value| !value.trim().is_empty())
        .map(escape_field)
        .unwrap_or_else(|| "-".to_string())
}

impl VolumeIndexPlan {
    pub fn included_roots(&self) -> Vec<PathBuf> {
        self.decisions
            .iter()
            .filter(|decision| decision.should_index())
            .map(|decision| decision.path.clone())
            .collect()
    }

    pub fn included_volume_scope(&self) -> SearchVolumeScope {
        let included = self
            .decisions
            .iter()
            .filter(|decision| decision.should_index())
            .collect::<Vec<_>>();
        if included.is_empty() {
            return SearchVolumeScope::only([]);
        }
        let volumes = included
            .iter()
            .filter_map(|decision| decision.id)
            .collect::<Vec<_>>();
        if volumes.len() == included.len() {
            SearchVolumeScope::only(volumes)
        } else {
            SearchVolumeScope::All
        }
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
