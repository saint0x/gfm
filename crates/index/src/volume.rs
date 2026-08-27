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

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "system" => Ok(Self::System),
            "internal" => Ok(Self::Internal),
            "external" | "removable" | "disk-image" => Ok(Self::External),
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
    pub id: Option<VolumeId>,
    pub label: String,
    pub path: PathBuf,
    pub class: IndexVolumeClass,
    pub mount_state: IndexMountState,
    pub reachable: Option<bool>,
    pub read_only: Option<bool>,
    pub stable_identity: Option<String>,
    pub filesystem_signature: Option<String>,
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
            stable_identity: None,
            filesystem_signature: None,
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

    pub fn with_filesystem_signature(mut self, filesystem_signature: impl Into<String>) -> Self {
        self.filesystem_signature = Some(filesystem_signature.into());
        self
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
    pub current_class: Option<IndexVolumeClass>,
    pub current_mount_state: Option<IndexMountState>,
    pub current_reachable: Option<bool>,
    pub current_read_only: Option<bool>,
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
    pub current_volume_id: Option<VolumeId>,
    pub current_class: Option<IndexVolumeClass>,
    pub current_mount_state: Option<IndexMountState>,
    pub current_reachable: Option<bool>,
    pub current_read_only: Option<bool>,
    pub read_only_changed: bool,
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
        let current_class = current.map(|volume| volume.class);
        let current_mount_state = current.map(|volume| volume.mount_state);
        let current_reachable = current.and_then(|volume| volume.reachable);
        let current_read_only = current.and_then(|volume| volume.read_only);

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
                if known_optional_value_changed(&previous.reachable, &current.reachable) =>
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
                if known_optional_value_changed(&previous.read_only, &current.read_only) =>
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
                if known_optional_value_changed(
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
                if known_optional_value_changed(
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
            current_class,
            current_mount_state,
            current_reachable,
            current_read_only,
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
            "volume-invalidation\tpath={}\tprevious-class={}\tprevious-mount={}\tprevious-reachable={}\tprevious-read-only={}\tcurrent-class={}\tcurrent-mount={}\tcurrent-reachable={}\tcurrent-read-only={}\tsidebar={}\toperation-policy={}\tindex-admission={}\trescan-index={}\tcancel-index-jobs={}\tclear-fsevents-cursor={}\treason={}",
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
        let stable_identity_changed = known_optional_value_changed(
            &previous.and_then(|volume| volume.stable_identity.clone()),
            &current.and_then(|volume| volume.stable_identity.clone()),
        );
        let filesystem_signature_changed = known_optional_value_changed(
            &previous.and_then(|volume| volume.filesystem_signature.clone()),
            &current.and_then(|volume| volume.filesystem_signature.clone()),
        );
        let read_only_changed = known_optional_value_changed(
            &previous.and_then(|volume| volume.read_only),
            &current.and_then(|volume| volume.read_only),
        );
        let event_visible = path.is_some()
            || previous.is_some()
            || current.is_some()
            || source_invalidates_index_admission;
        let descriptor_changed =
            stable_identity_changed || filesystem_signature_changed || read_only_changed;
        let invalidate_index_admission =
            event_visible && (source_invalidates_index_admission || descriptor_changed);
        let rescan_index = event_visible && (source_rescans_index || descriptor_changed);
        let cancel_index_jobs = event_visible
            && (descriptor_changed
                || matches!(
                    kind,
                    IndexVolumeEventKind::DescriptionChanged
                        | IndexVolumeEventKind::Disappeared
                        | IndexVolumeEventKind::Unavailable
                ));
        let clear_fsevents_cursor = invalidate_index_admission || rescan_index || cancel_index_jobs;
        let reason = match kind {
            IndexVolumeEventKind::Appeared if current.is_some() => "volume-event-connected",
            IndexVolumeEventKind::Appeared => "volume-event-appeared-unclassified",
            IndexVolumeEventKind::DescriptionChanged if stable_identity_changed => {
                "volume-event-identity-changed"
            }
            IndexVolumeEventKind::DescriptionChanged if filesystem_signature_changed => {
                "volume-event-filesystem-changed"
            }
            IndexVolumeEventKind::DescriptionChanged if read_only_changed => {
                "volume-event-read-only-changed"
            }
            IndexVolumeEventKind::DescriptionChanged if current.is_some() => {
                "volume-event-descriptor-changed"
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
            current_volume_id: current.and_then(|volume| volume.id),
            current_class: current.map(|volume| volume.class),
            current_mount_state: current.map(|volume| volume.mount_state),
            current_reachable: current.and_then(|volume| volume.reachable),
            current_read_only: current.and_then(|volume| volume.read_only),
            read_only_changed,
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
            "volume-event-index-invalidation\tkind={}\tpath={}\tprevious-volume={}\tprevious-class={}\tprevious-mount={}\tprevious-reachable={}\tprevious-read-only={}\tcurrent-volume={}\tcurrent-class={}\tcurrent-mount={}\tcurrent-reachable={}\tcurrent-read-only={}\tread-only-changed={}\tidentity-changed={}\tfilesystem-changed={}\tindex-admission={}\trescan-index={}\tcancel-index-jobs={}\tclear-fsevents-cursor={}\treason={}",
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
            escape_field(&self.reason)
        )
    }
}

fn known_optional_value_changed<T: Eq>(previous: &Option<T>, current: &Option<T>) -> bool {
    matches!((previous, current), (Some(previous), Some(current)) if previous != current)
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
