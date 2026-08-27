use gfm_mac_sys::{
    NativeVolumeDescription, NativeVolumeMountTableEntry, NativeVolumeOperation,
    NativeVolumeOperationStatus, NativeVolumeResourceValues, NativeVolumeStatus,
};
use gfm_types::{GfmError, Result, VolumeId};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

const VOLUME_MARKER: &str = ".gfm-volume-kind";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeKind {
    System,
    Internal,
    External,
    Removable,
    Network,
    DiskImage,
    Unknown,
}

impl VolumeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Internal => "internal",
            Self::External => "external",
            Self::Removable => "removable",
            Self::Network => "network",
            Self::DiskImage => "disk-image",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountState {
    Mounted,
    Unmounted,
    Stale,
}

impl MountState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mounted => "mounted",
            Self::Unmounted => "unmounted",
            Self::Stale => "stale",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeCommandState {
    Enabled,
    Disabled,
    Hidden,
}

impl VolumeCommandState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::Hidden => "hidden",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeCommandPolicy {
    pub eject: VolumeCommandState,
    pub mount: VolumeCommandState,
    pub unmount: VolumeCommandState,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeCapacity {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

impl VolumeCapacity {
    fn read(path: &Path) -> Self {
        let total_bytes = fs2::total_space(path).unwrap_or(0);
        let available_bytes = fs2::available_space(path).unwrap_or(0);
        Self {
            total_bytes,
            available_bytes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeDescriptor {
    pub id: VolumeId,
    pub stable_identity: String,
    pub label: String,
    pub path: PathBuf,
    pub kind: VolumeKind,
    pub mount_state: MountState,
    pub removable: bool,
    pub network: bool,
    pub reachable: Option<bool>,
    pub ejectable: bool,
    pub writable: bool,
    pub read_only: bool,
    pub case_sensitive: Option<bool>,
    pub case_preserving: Option<bool>,
    pub local: Option<bool>,
    pub internal: Option<bool>,
    pub mountable: Option<bool>,
    pub capacity: VolumeCapacity,
    pub commands: VolumeCommandPolicy,
    pub native_status: Option<NativeVolumeStatus>,
    pub resource_status: Option<NativeVolumeStatus>,
    pub resource_uuid: Option<String>,
    pub resource_automounted: Option<bool>,
    pub resource_browsable: Option<bool>,
    pub resource_reachable: Option<bool>,
    pub resource_remount_url: Option<String>,
    pub mount_table_status: Option<NativeVolumeStatus>,
    pub mount_from: Option<String>,
    pub mount_filesystem: Option<String>,
    pub mount_flags: Option<u32>,
    pub mount_read_only: Option<bool>,
    pub mount_local: Option<bool>,
    pub bsd_name: Option<String>,
    pub volume_uuid: Option<String>,
    pub volume_type: Option<String>,
    pub media_uuid: Option<String>,
    pub filesystem: Option<String>,
    pub media_content: Option<String>,
    pub media_kind: Option<String>,
    pub media_name: Option<String>,
    pub media_path: Option<String>,
    pub media_type: Option<String>,
    pub media_leaf: Option<bool>,
    pub media_whole: Option<bool>,
    pub media_encrypted: Option<bool>,
    pub media_block_size_bytes: Option<u64>,
    pub media_size_bytes: Option<u64>,
    pub device_protocol: Option<String>,
    pub device_model: Option<String>,
    pub device_path: Option<String>,
    pub device_vendor: Option<String>,
    pub source: String,
}

impl VolumeDescriptor {
    pub fn for_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let metadata = fs::metadata(&path).map_err(|err| GfmError::io(&path, err))?;
        let id = volume_id(&metadata);
        let marker = marker_kind(&path);
        let native = Some(gfm_mac_sys::copy_volume_description_for_path(&path));
        let resource = Some(gfm_mac_sys::copy_volume_resource_values(&path));
        let mount_table = Some(gfm_mac_sys::copy_volume_mount_table_entry(&path));
        let native_status = native.as_ref().map(|native| native.status);
        let resource_status = resource.as_ref().map(|resource| resource.status);
        let mount_table_status = mount_table.as_ref().map(|mount_table| mount_table.status);
        let label = native
            .as_ref()
            .and_then(|native| native.volume_name.clone())
            .unwrap_or_else(|| volume_label(&path));
        let kind = classify_volume(
            &path,
            marker.as_deref(),
            native.as_ref(),
            resource.as_ref(),
            mount_table.as_ref(),
        );
        let mount_state = if path.exists() {
            MountState::Mounted
        } else {
            MountState::Stale
        };
        let marker_value = marker.as_deref();
        let removable = marker_removable(marker_value)
            .or_else(|| resource.as_ref().and_then(|resource| resource.is_removable))
            .or_else(|| native.as_ref().and_then(|native| native.media_removable))
            .unwrap_or({
                matches!(
                    kind,
                    VolumeKind::External | VolumeKind::Removable | VolumeKind::DiskImage
                )
            });
        let local = mount_table
            .as_ref()
            .and_then(|mount_table| mount_table.is_local)
            .or_else(|| resource.as_ref().and_then(|resource| resource.is_local));
        let network = marker_network(marker_value)
            .or_else(|| local.map(|local| !local))
            .or_else(|| native.as_ref().and_then(|native| native.volume_network))
            .unwrap_or(kind == VolumeKind::Network);
        let reachable = marker_reachability(
            marker.as_deref(),
            network,
            mount_state,
            resource.as_ref(),
            &path,
        );
        let ejectable = marker_ejectable(marker_value)
            .or_else(|| resource.as_ref().and_then(|resource| resource.is_ejectable))
            .or_else(|| native.as_ref().and_then(|native| native.media_ejectable))
            .unwrap_or(removable || network);
        let writable = mount_table
            .as_ref()
            .and_then(|mount_table| mount_table.is_read_only.map(|read_only| !read_only))
            .or_else(|| {
                resource
                    .as_ref()
                    .and_then(|resource| resource.is_read_only.map(|read_only| !read_only))
            })
            .or_else(|| native.as_ref().and_then(|native| native.media_writable))
            .unwrap_or_else(|| !metadata.permissions().readonly());
        let read_only = mount_table
            .as_ref()
            .and_then(|mount_table| mount_table.is_read_only)
            .or_else(|| resource.as_ref().and_then(|resource| resource.is_read_only))
            .unwrap_or(!writable);
        let case_sensitive = resource
            .as_ref()
            .and_then(|resource| resource.supports_case_sensitive_names);
        let case_preserving = resource
            .as_ref()
            .and_then(|resource| resource.supports_case_preserved_names);
        let internal = resource
            .as_ref()
            .and_then(|resource| resource.is_internal)
            .or_else(|| native.as_ref().and_then(|native| native.device_internal));
        let mountable = native.as_ref().and_then(|native| native.volume_mountable);
        let capacity = VolumeCapacity::read(&path);
        let commands = command_policy(kind, mount_state, ejectable);
        let stable_identity = match marker.as_deref() {
            Some(marker) => marker_stable_identity(marker, id, &path),
            None => stable_identity(
                id,
                &path,
                native.as_ref(),
                resource.as_ref(),
                mount_table.as_ref(),
            ),
        };
        let source = marker
            .map(|marker| format!("fixture-marker:{marker}"))
            .unwrap_or_else(|| volume_source(native.as_ref()));

        Ok(Self {
            id,
            stable_identity,
            label,
            path,
            kind,
            mount_state,
            removable,
            network,
            reachable,
            ejectable,
            writable,
            read_only,
            case_sensitive,
            case_preserving,
            local,
            internal,
            mountable,
            capacity,
            commands,
            native_status,
            resource_status,
            resource_uuid: resource
                .as_ref()
                .and_then(|resource| resource.volume_uuid.clone()),
            resource_automounted: resource
                .as_ref()
                .and_then(|resource| resource.is_automounted),
            resource_browsable: resource.as_ref().and_then(|resource| resource.is_browsable),
            resource_reachable: resource.as_ref().and_then(|resource| resource.is_reachable),
            resource_remount_url: resource
                .as_ref()
                .and_then(|resource| resource.remount_url.clone()),
            mount_table_status,
            mount_from: mount_table
                .as_ref()
                .and_then(|mount_table| mount_table.mounted_from.clone()),
            mount_filesystem: mount_table
                .as_ref()
                .and_then(|mount_table| mount_table.filesystem_type.clone()),
            mount_flags: mount_table
                .as_ref()
                .and_then(|mount_table| mount_table.flags),
            mount_read_only: mount_table
                .as_ref()
                .and_then(|mount_table| mount_table.is_read_only),
            mount_local: mount_table
                .as_ref()
                .and_then(|mount_table| mount_table.is_local),
            bsd_name: native
                .as_ref()
                .and_then(|native| native.media_bsd_name.clone()),
            volume_uuid: native
                .as_ref()
                .and_then(|native| native.volume_uuid.clone()),
            volume_type: native
                .as_ref()
                .and_then(|native| native.volume_type.clone()),
            media_uuid: native.as_ref().and_then(|native| native.media_uuid.clone()),
            filesystem: mount_table
                .as_ref()
                .and_then(|mount_table| mount_table.filesystem_type.clone())
                .or_else(|| {
                    native.as_ref().and_then(|native| {
                        native
                            .volume_kind
                            .clone()
                            .or_else(|| native.volume_type.clone())
                    })
                }),
            media_content: native
                .as_ref()
                .and_then(|native| native.media_content.clone()),
            media_kind: native.as_ref().and_then(|native| native.media_kind.clone()),
            media_name: native.as_ref().and_then(|native| native.media_name.clone()),
            media_path: native.as_ref().and_then(|native| native.media_path.clone()),
            media_type: native.as_ref().and_then(|native| native.media_type.clone()),
            media_leaf: native.as_ref().and_then(|native| native.media_leaf),
            media_whole: native.as_ref().and_then(|native| native.media_whole),
            media_encrypted: native.as_ref().and_then(|native| native.media_encrypted),
            media_block_size_bytes: native
                .as_ref()
                .and_then(|native| native.media_block_size_bytes),
            media_size_bytes: native.as_ref().and_then(|native| native.media_size_bytes),
            device_protocol: native
                .as_ref()
                .and_then(|native| native.device_protocol.clone()),
            device_model: native
                .as_ref()
                .and_then(|native| native.device_model.clone()),
            device_path: native
                .as_ref()
                .and_then(|native| native.device_path.clone()),
            device_vendor: native
                .as_ref()
                .and_then(|native| native.device_vendor.clone()),
            source: enrich_volume_source(
                source,
                resource_status,
                resource.as_ref(),
                mount_table_status,
                mount_table.as_ref(),
            ),
        })
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "volume\t{}\t{}\tpath={}\tkind={}\tmount={}\tremovable={}\tnetwork={}\treachable={}\tejectable={}\ttotal={}\tavailable={}\teject={}\tmount={}\tunmount={}\tsource={}\treason={}\tstable-id={}\tnative-status={}\twritable={}\tread-only={}\tcase-sensitive={}\tcase-preserving={}\tlocal={}\tinternal={}\tmountable={}\tbsd={}\tvolume-uuid={}\tmedia-uuid={}\tfs={}\tmedia-content={}\tprotocol={}\tmodel={}\tvendor={}\tresource-status={}\tresource-uuid={}\tresource-automounted={}\tresource-browsable={}\tresource-reachable={}\tresource-remount-url={}\tmount-status={}\tmount-from={}\tmount-fs={}\tmount-flags={}\tmount-read-only={}\tmount-local={}\tvolume-type={}\tmedia-kind={}\tmedia-name={}\tmedia-path={}\tmedia-type={}\tmedia-leaf={}\tmedia-whole={}\tmedia-encrypted={}\tmedia-block-size={}\tmedia-size={}\tdevice-path={}",
            self.id.0,
            escape_field(&self.label),
            self.path.display(),
            self.kind.as_str(),
            self.mount_state.as_str(),
            self.removable,
            self.network,
            self.reachable
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.ejectable,
            self.capacity.total_bytes,
            self.capacity.available_bytes,
            self.commands.eject.as_str(),
            self.commands.mount.as_str(),
            self.commands.unmount.as_str(),
            escape_field(&self.source),
            self.commands
                .reason
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            escape_field(&self.stable_identity),
            self.native_status
                .map(NativeVolumeStatus::as_str)
                .unwrap_or("-"),
            self.writable,
            self.read_only,
            self.case_sensitive
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.case_preserving
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.local
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.internal
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.mountable
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.bsd_name
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            self.volume_uuid
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            self.media_uuid
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            self.filesystem
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            self.media_content
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            self.device_protocol
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            self.device_model
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            self.device_vendor
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            self.resource_status
                .map(NativeVolumeStatus::as_str)
                .unwrap_or("-"),
            self.resource_uuid
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            self.resource_automounted
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.resource_browsable
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.resource_reachable
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.resource_remount_url
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            self.mount_table_status
                .map(NativeVolumeStatus::as_str)
                .unwrap_or("-"),
            self.mount_from
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            self.mount_filesystem
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            self.mount_flags
                .map(|flags| flags.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.mount_read_only
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.mount_local
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.volume_type
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            self.media_kind
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            self.media_name
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            self.media_path
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            self.media_type
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            self.media_leaf
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.media_whole
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.media_encrypted
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.media_block_size_bytes
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.media_size_bytes
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.device_path
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string())
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeDiscoveryReport {
    pub volumes: Vec<VolumeDescriptor>,
}

impl VolumeDiscoveryReport {
    pub fn discover() -> Self {
        let mut paths = mounted_volume_paths();
        if paths.is_empty() {
            paths = fallback_volume_paths();
        }
        Self::from_paths(paths)
    }

    pub fn from_paths(paths: Vec<PathBuf>) -> Self {
        let mut volumes: Vec<_> = paths
            .into_iter()
            .filter_map(|path| VolumeDescriptor::for_path(path).ok())
            .collect();
        volumes.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then(left.label.cmp(&right.label))
                .then(left.id.cmp(&right.id))
        });
        volumes.dedup_by(|left, right| left.id == right.id && left.path == right.path);
        Self { volumes }
    }

    pub fn for_containing_path(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let mut paths = containing_mounted_volume_paths(path);
        paths.extend(marker_ancestor_volume_paths(path));
        if paths.is_empty() {
            paths = fallback_volume_paths();
        }
        Self::from_paths(paths)
    }

    pub fn volume_for_path(&self, path: &Path) -> Option<&VolumeDescriptor> {
        self.volumes
            .iter()
            .filter(|volume| path.starts_with(&volume.path))
            .max_by_key(|volume| volume.path.components().count())
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = vec![format!("volumes\tcount={}", self.volumes.len())];
        lines.extend(self.volumes.iter().map(VolumeDescriptor::as_tsv));
        lines.join("\n")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeTopologyChangeKind {
    Connected,
    Disconnected,
    Changed,
}

impl VolumeTopologyChangeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Disconnected => "disconnected",
            Self::Changed => "changed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeTopologyChange {
    pub kind: VolumeTopologyChangeKind,
    pub stable_identity: String,
    pub label: String,
    pub path: PathBuf,
    pub previous_kind: Option<VolumeKind>,
    pub current_kind: Option<VolumeKind>,
    pub previous_mount_state: Option<MountState>,
    pub current_mount_state: Option<MountState>,
    pub invalidate_sidebar: bool,
    pub invalidate_operation_policy: bool,
    pub invalidate_index_admission: bool,
    pub rescan_index: bool,
    pub reason: String,
}

impl VolumeTopologyChange {
    fn connected(volume: &VolumeDescriptor) -> Self {
        Self {
            kind: VolumeTopologyChangeKind::Connected,
            stable_identity: volume.stable_identity.clone(),
            label: volume.label.clone(),
            path: volume.path.clone(),
            previous_kind: None,
            current_kind: Some(volume.kind),
            previous_mount_state: None,
            current_mount_state: Some(volume.mount_state),
            invalidate_sidebar: true,
            invalidate_operation_policy: true,
            invalidate_index_admission: true,
            rescan_index: true,
            reason: "volume-connected".to_string(),
        }
    }

    fn disconnected(volume: &VolumeDescriptor) -> Self {
        Self {
            kind: VolumeTopologyChangeKind::Disconnected,
            stable_identity: volume.stable_identity.clone(),
            label: volume.label.clone(),
            path: volume.path.clone(),
            previous_kind: Some(volume.kind),
            current_kind: None,
            previous_mount_state: Some(volume.mount_state),
            current_mount_state: None,
            invalidate_sidebar: true,
            invalidate_operation_policy: true,
            invalidate_index_admission: true,
            rescan_index: true,
            reason: "volume-disconnected".to_string(),
        }
    }

    fn changed(previous: &VolumeDescriptor, current: &VolumeDescriptor) -> Option<Self> {
        let reason = topology_change_reason(previous, current)?;
        let invalidates_policy = matches!(
            reason,
            "volume-path-changed"
                | "mount-state-changed"
                | "volume-kind-changed"
                | "volume-access-changed"
                | "volume-locality-changed"
                | "volume-ejectability-changed"
                | "volume-identity-changed"
                | "volume-filesystem-changed"
        );
        let rescan_index = matches!(
            reason,
            "volume-path-changed"
                | "mount-state-changed"
                | "volume-kind-changed"
                | "volume-access-changed"
                | "volume-locality-changed"
                | "volume-identity-changed"
                | "volume-filesystem-changed"
        );
        Some(Self {
            kind: VolumeTopologyChangeKind::Changed,
            stable_identity: current.stable_identity.clone(),
            label: current.label.clone(),
            path: current.path.clone(),
            previous_kind: Some(previous.kind),
            current_kind: Some(current.kind),
            previous_mount_state: Some(previous.mount_state),
            current_mount_state: Some(current.mount_state),
            invalidate_sidebar: true,
            invalidate_operation_policy: invalidates_policy,
            invalidate_index_admission: invalidates_policy,
            rescan_index,
            reason: reason.to_string(),
        })
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "volume-topology\t{}\tstable-id={}\tlabel={}\tpath={}\tprevious-kind={}\tcurrent-kind={}\tprevious-mount={}\tcurrent-mount={}\tsidebar={}\toperation-policy={}\tindex-admission={}\trescan-index={}\treason={}",
            self.kind.as_str(),
            escape_field(&self.stable_identity),
            escape_field(&self.label),
            self.path.display(),
            self.previous_kind.map(VolumeKind::as_str).unwrap_or("-"),
            self.current_kind.map(VolumeKind::as_str).unwrap_or("-"),
            self.previous_mount_state
                .map(MountState::as_str)
                .unwrap_or("-"),
            self.current_mount_state.map(MountState::as_str).unwrap_or("-"),
            self.invalidate_sidebar,
            self.invalidate_operation_policy,
            self.invalidate_index_admission,
            self.rescan_index,
            escape_field(&self.reason)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeTopologyDiff {
    pub changes: Vec<VolumeTopologyChange>,
}

impl VolumeTopologyDiff {
    pub fn evaluate(previous: &VolumeDiscoveryReport, current: &VolumeDiscoveryReport) -> Self {
        let previous = volume_map(&previous.volumes);
        let current = volume_map(&current.volumes);
        let mut changes = Vec::new();

        for (stable_identity, previous_volume) in &previous {
            match current.get(stable_identity) {
                Some(current_volume) => {
                    if let Some(change) =
                        VolumeTopologyChange::changed(previous_volume, current_volume)
                    {
                        changes.push(change);
                    }
                }
                None => changes.push(VolumeTopologyChange::disconnected(previous_volume)),
            }
        }
        for (stable_identity, current_volume) in &current {
            if !previous.contains_key(stable_identity) {
                changes.push(VolumeTopologyChange::connected(current_volume));
            }
        }
        changes.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then(left.stable_identity.cmp(&right.stable_identity))
        });
        Self { changes }
    }

    pub fn as_tsv(&self) -> String {
        let mut lines = vec![format!(
            "volume-topology-diff\tcount={}",
            self.changes.len()
        )];
        lines.extend(self.changes.iter().map(VolumeTopologyChange::as_tsv));
        lines.join("\n")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeEventKind {
    Appeared,
    DescriptionChanged,
    Disappeared,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeEventReport {
    pub kind: VolumeEventKind,
    pub native_status: NativeVolumeStatus,
    pub path: Option<PathBuf>,
    pub descriptor: Option<VolumeDescriptor>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeEventInvalidationReport {
    pub kind: VolumeEventKind,
    pub native_status: NativeVolumeStatus,
    pub path: Option<PathBuf>,
    pub previous_kind: Option<VolumeKind>,
    pub previous_mount_state: Option<MountState>,
    pub current_kind: Option<VolumeKind>,
    pub current_mount_state: Option<MountState>,
    pub invalidate_sidebar: bool,
    pub invalidate_operation_policy: bool,
    pub invalidate_index_admission: bool,
    pub rescan_index: bool,
    pub reason: String,
}

pub struct VolumeEventStream {
    stream: gfm_mac_sys::NativeVolumeEventStream,
}

impl VolumeEventKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Appeared => "appeared",
            Self::DescriptionChanged => "description-changed",
            Self::Disappeared => "disappeared",
            Self::Unavailable => "unavailable",
        }
    }
}

impl From<gfm_mac_sys::NativeVolumeEventKind> for VolumeEventKind {
    fn from(kind: gfm_mac_sys::NativeVolumeEventKind) -> Self {
        match kind {
            gfm_mac_sys::NativeVolumeEventKind::Appeared => Self::Appeared,
            gfm_mac_sys::NativeVolumeEventKind::DescriptionChanged => Self::DescriptionChanged,
            gfm_mac_sys::NativeVolumeEventKind::Disappeared => Self::Disappeared,
            gfm_mac_sys::NativeVolumeEventKind::Unavailable => Self::Unavailable,
        }
    }
}

impl VolumeEventReport {
    fn from_native(event: gfm_mac_sys::NativeVolumeEvent) -> Self {
        let path = event.description.volume_path.clone();
        let descriptor = path
            .as_ref()
            .filter(|path| path.exists())
            .and_then(|path| VolumeDescriptor::for_path(path).ok());
        Self {
            kind: event.kind.into(),
            native_status: event.description.status,
            path,
            descriptor,
            reason: event.description.reason,
        }
    }

    pub fn as_tsv(&self) -> String {
        let descriptor = self
            .descriptor
            .as_ref()
            .map(VolumeDescriptor::as_tsv)
            .unwrap_or_else(|| "volume=-".to_string());
        format!(
            "volume-event\tkind={}\tnative-status={}\tpath={}\treason={}\n{}",
            self.kind.as_str(),
            self.native_status.as_str(),
            self.path
                .as_ref()
                .map(|path| escape_field(&path.to_string_lossy()))
                .unwrap_or_else(|| "-".to_string()),
            self.reason
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            descriptor
        )
    }
}

impl VolumeEventInvalidationReport {
    pub fn from_event(event: &VolumeEventReport) -> Self {
        Self::from_parts(
            event.kind,
            event.native_status,
            event.path.clone(),
            event.descriptor.as_ref(),
            event.reason.clone(),
        )
    }

    pub fn from_parts(
        kind: VolumeEventKind,
        native_status: NativeVolumeStatus,
        path: Option<PathBuf>,
        descriptor: Option<&VolumeDescriptor>,
        native_reason: Option<String>,
    ) -> Self {
        let descriptor_visible = descriptor.is_some();
        let event_visible =
            descriptor_visible || path.is_some() || native_status != NativeVolumeStatus::Available;
        let invalidates = event_visible
            && matches!(
                kind,
                VolumeEventKind::Appeared
                    | VolumeEventKind::DescriptionChanged
                    | VolumeEventKind::Disappeared
                    | VolumeEventKind::Unavailable
            );
        let reason = match kind {
            VolumeEventKind::Appeared => "volume-event-appeared",
            VolumeEventKind::DescriptionChanged => "volume-event-description-changed",
            VolumeEventKind::Disappeared => "volume-event-disappeared",
            VolumeEventKind::Unavailable => native_reason
                .as_deref()
                .unwrap_or("volume-event-unavailable"),
        };
        Self {
            kind,
            native_status,
            path,
            previous_kind: (kind == VolumeEventKind::Disappeared)
                .then(|| descriptor.map(|descriptor| descriptor.kind))
                .flatten(),
            previous_mount_state: (kind == VolumeEventKind::Disappeared)
                .then(|| descriptor.map(|descriptor| descriptor.mount_state))
                .flatten(),
            current_kind: (kind != VolumeEventKind::Disappeared)
                .then(|| descriptor.map(|descriptor| descriptor.kind))
                .flatten(),
            current_mount_state: if kind == VolumeEventKind::Disappeared && event_visible {
                Some(MountState::Unmounted)
            } else {
                descriptor.map(|descriptor| descriptor.mount_state)
            },
            invalidate_sidebar: invalidates,
            invalidate_operation_policy: invalidates,
            invalidate_index_admission: invalidates,
            rescan_index: invalidates,
            reason: reason.to_string(),
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "volume-event-invalidation\tkind={}\tnative-status={}\tpath={}\tprevious-kind={}\tprevious-mount={}\tcurrent-kind={}\tcurrent-mount={}\tsidebar={}\toperation-policy={}\tindex-admission={}\trescan-index={}\treason={}",
            self.kind.as_str(),
            self.native_status.as_str(),
            self.path
                .as_ref()
                .map(|path| escape_field(&path.to_string_lossy()))
                .unwrap_or_else(|| "-".to_string()),
            self.previous_kind.map(VolumeKind::as_str).unwrap_or("-"),
            self.previous_mount_state.map(MountState::as_str).unwrap_or("-"),
            self.current_kind.map(VolumeKind::as_str).unwrap_or("-"),
            self.current_mount_state.map(MountState::as_str).unwrap_or("-"),
            self.invalidate_sidebar,
            self.invalidate_operation_policy,
            self.invalidate_index_admission,
            self.rescan_index,
            escape_field(&self.reason)
        )
    }
}

impl VolumeEventStream {
    pub fn start() -> Self {
        Self {
            stream: gfm_mac_sys::NativeVolumeEventStream::start(),
        }
    }

    pub fn is_attached(&self) -> bool {
        self.stream.is_attached()
    }

    pub fn try_recv(&self) -> Option<VolumeEventReport> {
        self.stream.try_recv().map(VolumeEventReport::from_native)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeOperation {
    Eject,
    Unmount,
    Mount,
}

impl VolumeOperation {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "eject" => Ok(Self::Eject),
            "unmount" => Ok(Self::Unmount),
            "mount" => Ok(Self::Mount),
            other => Err(GfmError::Format(format!(
                "invalid volume operation `{other}`; expected eject, unmount, or mount"
            ))),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Eject => "eject",
            Self::Unmount => "unmount",
            Self::Mount => "mount",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeOperationDisposition {
    Completed,
    Submitted,
    Refused,
    Busy,
    Denied,
    Unsupported,
    Failed,
}

impl VolumeOperationDisposition {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Submitted => "submitted",
            Self::Refused => "refused",
            Self::Busy => "busy",
            Self::Denied => "denied",
            Self::Unsupported => "unsupported",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeOperationReport {
    pub path: PathBuf,
    pub operation: VolumeOperation,
    pub disposition: VolumeOperationDisposition,
    pub native_status: Option<NativeVolumeOperationStatus>,
    pub dissenter_status: Option<u32>,
    pub volume: Option<VolumeDescriptor>,
    pub reason: String,
}

impl VolumeOperationReport {
    pub fn execute(path: impl AsRef<Path>, operation: VolumeOperation) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if !path.exists() {
            return Ok(Self::without_volume(
                path,
                operation,
                VolumeOperationDisposition::Refused,
                "volume-path-missing",
            ));
        }

        let volume = VolumeDescriptor::for_path(&path)?;
        if operation == VolumeOperation::Mount {
            return Ok(Self::with_volume(
                operation,
                VolumeOperationDisposition::Unsupported,
                None,
                None,
                volume,
                "native-mount-requires-unmounted-disk-identity",
            ));
        }
        if volume.source.starts_with("fixture-marker:") {
            return Ok(Self::with_volume(
                operation,
                VolumeOperationDisposition::Refused,
                None,
                None,
                volume,
                "fixture-volume-native-operation-disabled",
            ));
        }
        if path == Path::new("/") {
            return Ok(Self::with_volume(
                operation,
                VolumeOperationDisposition::Refused,
                None,
                None,
                volume,
                "system-volume-operation-refused",
            ));
        }
        if !path.starts_with("/Volumes") {
            return Ok(Self::with_volume(
                operation,
                VolumeOperationDisposition::Refused,
                None,
                None,
                volume,
                "native-volume-operation-requires-volumes-root",
            ));
        }
        if volume.native_status != Some(NativeVolumeStatus::Available) {
            return Ok(Self::with_volume(
                operation,
                VolumeOperationDisposition::Refused,
                None,
                None,
                volume,
                "diskarbitration-volume-unavailable",
            ));
        }
        if let Some(reason) = disabled_command_reason(operation, &volume) {
            return Ok(Self::with_volume(
                operation,
                VolumeOperationDisposition::Refused,
                None,
                None,
                volume,
                reason,
            ));
        }

        let native_operation = match operation {
            VolumeOperation::Eject => NativeVolumeOperation::Eject,
            VolumeOperation::Unmount => NativeVolumeOperation::Unmount,
            VolumeOperation::Mount => unreachable!("mount is handled before native submission"),
        };
        let native = gfm_mac_sys::submit_volume_operation(&path, native_operation);
        let disposition = disposition_for_native_operation(native.status);
        Ok(Self::with_volume(
            operation,
            disposition,
            Some(native.status),
            native.dissenter_status,
            volume,
            native.reason.as_deref().unwrap_or(native.status.as_str()),
        ))
    }

    fn without_volume(
        path: PathBuf,
        operation: VolumeOperation,
        disposition: VolumeOperationDisposition,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            path,
            operation,
            disposition,
            native_status: None,
            dissenter_status: None,
            volume: None,
            reason: reason.into(),
        }
    }

    fn with_volume(
        operation: VolumeOperation,
        disposition: VolumeOperationDisposition,
        native_status: Option<NativeVolumeOperationStatus>,
        dissenter_status: Option<u32>,
        volume: VolumeDescriptor,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            path: volume.path.clone(),
            operation,
            disposition,
            native_status,
            dissenter_status,
            volume: Some(volume),
            reason: reason.into(),
        }
    }

    pub fn as_tsv(&self) -> String {
        let (kind, mount, stable_identity) = self
            .volume
            .as_ref()
            .map(|volume| {
                (
                    volume.kind.as_str(),
                    volume.mount_state.as_str(),
                    escape_field(&volume.stable_identity),
                )
            })
            .unwrap_or(("-", "-", "-".to_string()));
        format!(
            "volume-operation\t{}\tpath={}\tdisposition={}\tnative-status={}\tdissenter-status={}\tvolume-kind={}\tmount={}\tstable-id={}\treason={}",
            self.operation.as_str(),
            self.path.display(),
            self.disposition.as_str(),
            self.native_status
                .map(NativeVolumeOperationStatus::as_str)
                .unwrap_or("-"),
            self.dissenter_status
                .map(|status| format!("0x{status:08x}"))
                .unwrap_or_else(|| "-".to_string()),
            kind,
            mount,
            stable_identity,
            escape_field(&self.reason)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeMountIdentityReport {
    pub bsd_name: String,
    pub disposition: VolumeOperationDisposition,
    pub native_status: NativeVolumeOperationStatus,
    pub dissenter_status: Option<u32>,
    pub reason: String,
}

impl VolumeMountIdentityReport {
    pub fn execute(bsd_name: impl Into<String>) -> Self {
        let bsd_name = bsd_name.into();
        let native = gfm_mac_sys::submit_volume_mount_by_bsd_name(&bsd_name);
        let disposition = disposition_for_native_operation(native.status);
        Self {
            bsd_name,
            disposition,
            native_status: native.status,
            dissenter_status: native.dissenter_status,
            reason: native
                .reason
                .unwrap_or_else(|| native.status.as_str().to_string()),
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "volume-mount-bsd\tbsd-name={}\tdisposition={}\tnative-status={}\tdissenter-status={}\treason={}",
            escape_field(&self.bsd_name),
            self.disposition.as_str(),
            self.native_status.as_str(),
            self.dissenter_status
                .map(|status| format!("0x{status:08x}"))
                .unwrap_or_else(|| "-".to_string()),
            escape_field(&self.reason)
        )
    }
}

fn disposition_for_native_operation(
    status: NativeVolumeOperationStatus,
) -> VolumeOperationDisposition {
    match status {
        NativeVolumeOperationStatus::Succeeded => VolumeOperationDisposition::Completed,
        NativeVolumeOperationStatus::Submitted => VolumeOperationDisposition::Submitted,
        NativeVolumeOperationStatus::Busy => VolumeOperationDisposition::Busy,
        NativeVolumeOperationStatus::NotPermitted | NativeVolumeOperationStatus::NotPrivileged => {
            VolumeOperationDisposition::Denied
        }
        NativeVolumeOperationStatus::Unsupported => VolumeOperationDisposition::Unsupported,
        NativeVolumeOperationStatus::NotMounted => VolumeOperationDisposition::Refused,
        NativeVolumeOperationStatus::Failed
        | NativeVolumeOperationStatus::Missing
        | NativeVolumeOperationStatus::Unavailable => VolumeOperationDisposition::Failed,
    }
}

fn disabled_command_reason(
    operation: VolumeOperation,
    volume: &VolumeDescriptor,
) -> Option<&'static str> {
    let state = match operation {
        VolumeOperation::Eject => volume.commands.eject,
        VolumeOperation::Unmount => volume.commands.unmount,
        VolumeOperation::Mount => volume.commands.mount,
    };
    (state != VolumeCommandState::Enabled).then_some(match operation {
        VolumeOperation::Eject => "eject-command-disabled",
        VolumeOperation::Unmount => "unmount-command-disabled",
        VolumeOperation::Mount => "mount-command-disabled",
    })
}

fn volume_map(volumes: &[VolumeDescriptor]) -> BTreeMap<String, &VolumeDescriptor> {
    volumes
        .iter()
        .map(|volume| (volume.stable_identity.clone(), volume))
        .collect()
}

fn topology_change_reason(
    previous: &VolumeDescriptor,
    current: &VolumeDescriptor,
) -> Option<&'static str> {
    if previous.path != current.path {
        Some("volume-path-changed")
    } else if previous.mount_state != current.mount_state {
        Some("mount-state-changed")
    } else if previous.kind != current.kind {
        Some("volume-kind-changed")
    } else if previous.read_only != current.read_only || previous.writable != current.writable {
        Some("volume-access-changed")
    } else if previous.network != current.network
        || previous.local != current.local
        || previous.reachable != current.reachable
    {
        Some("volume-locality-changed")
    } else if previous.ejectable != current.ejectable || previous.commands != current.commands {
        Some("volume-ejectability-changed")
    } else if previous.volume_uuid != current.volume_uuid
        || previous.media_uuid != current.media_uuid
        || previous.resource_uuid != current.resource_uuid
        || previous.bsd_name != current.bsd_name
        || previous.mount_from != current.mount_from
        || previous.media_content != current.media_content
        || previous.media_name != current.media_name
        || previous.media_path != current.media_path
    {
        Some("volume-identity-changed")
    } else if previous.filesystem != current.filesystem
        || previous.volume_type != current.volume_type
        || previous.media_kind != current.media_kind
        || previous.media_type != current.media_type
        || previous.media_leaf != current.media_leaf
        || previous.media_whole != current.media_whole
        || previous.media_encrypted != current.media_encrypted
        || previous.media_block_size_bytes != current.media_block_size_bytes
        || previous.media_size_bytes != current.media_size_bytes
        || previous.mount_filesystem != current.mount_filesystem
        || previous.mount_flags != current.mount_flags
        || previous.mount_local != current.mount_local
        || previous.case_sensitive != current.case_sensitive
        || previous.case_preserving != current.case_preserving
        || previous.resource_automounted != current.resource_automounted
        || previous.resource_browsable != current.resource_browsable
        || previous.resource_reachable != current.resource_reachable
        || previous.resource_remount_url != current.resource_remount_url
        || previous.device_protocol != current.device_protocol
        || previous.device_path != current.device_path
        || previous.device_model != current.device_model
        || previous.device_vendor != current.device_vendor
        || previous.internal != current.internal
    {
        Some("volume-filesystem-changed")
    } else if previous.label != current.label {
        Some("volume-label-changed")
    } else {
        None
    }
}

fn volume_reachability(
    network: bool,
    mount_state: MountState,
    resource: Option<&NativeVolumeResourceValues>,
    path: &Path,
) -> Option<bool> {
    if !network {
        return Some(mount_state == MountState::Mounted);
    }
    if mount_state != MountState::Mounted {
        return Some(false);
    }
    resource
        .filter(|resource| resource.status == NativeVolumeStatus::Available)
        .and_then(|resource| resource.is_reachable.or(resource.is_browsable))
        .or_else(|| Some(path.exists()))
}

fn marker_reachability(
    marker: Option<&str>,
    network: bool,
    mount_state: MountState,
    resource: Option<&NativeVolumeResourceValues>,
    path: &Path,
) -> Option<bool> {
    match marker {
        Some("network-unreachable") | Some("network-offline") => Some(false),
        _ => volume_reachability(network, mount_state, resource, path),
    }
}

fn classify_volume(
    path: &Path,
    marker: Option<&str>,
    native: Option<&gfm_mac_sys::NativeVolumeDescription>,
    resource: Option<&NativeVolumeResourceValues>,
    mount_table: Option<&NativeVolumeMountTableEntry>,
) -> VolumeKind {
    match marker {
        Some("network")
        | Some("network-smb")
        | Some("network-afp")
        | Some("network-nfs")
        | Some("network-unreachable")
        | Some("network-offline") => return VolumeKind::Network,
        Some("external") | Some("external-removable") => return VolumeKind::External,
        Some("removable") => return VolumeKind::Removable,
        Some("disk-image") => return VolumeKind::DiskImage,
        Some("system") => return VolumeKind::System,
        Some("internal") => return VolumeKind::Internal,
        _ => {}
    }

    if let Some(kind) = classify_native_volume(path, native, resource, mount_table) {
        return kind;
    }

    let label = volume_label(path).to_ascii_lowercase();
    if path == Path::new("/") || label == "macintosh hd" {
        VolumeKind::System
    } else if path.starts_with("/Network")
        || label.contains("smb")
        || label.contains("nfs")
        || label.contains("network")
    {
        VolumeKind::Network
    } else if path.starts_with("/Volumes") {
        VolumeKind::External
    } else {
        VolumeKind::Internal
    }
}

fn classify_native_volume(
    path: &Path,
    native: Option<&NativeVolumeDescription>,
    resource: Option<&NativeVolumeResourceValues>,
    mount_table: Option<&NativeVolumeMountTableEntry>,
) -> Option<VolumeKind> {
    if path == Path::new("/") {
        return Some(VolumeKind::System);
    }
    let native = native.filter(|native| native.status == NativeVolumeStatus::Available);
    if native.and_then(|native| native.volume_network) == Some(true) {
        return Some(VolumeKind::Network);
    }
    if let Some(native) = native {
        let protocol = native
            .device_protocol
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let media_kind = native
            .media_kind
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let volume_kind = native
            .volume_kind
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if protocol.contains("disk image")
            || media_kind.contains("disk image")
            || volume_kind.contains("disk image")
        {
            return Some(VolumeKind::DiskImage);
        }
    }

    let resource = resource.filter(|resource| resource.status == NativeVolumeStatus::Available);
    if resource.and_then(|resource| resource.is_local) == Some(false) {
        return Some(VolumeKind::Network);
    }
    if resource.and_then(|resource| resource.is_removable) == Some(true) {
        return Some(VolumeKind::Removable);
    }
    if resource.and_then(|resource| resource.is_ejectable) == Some(true)
        || resource.and_then(|resource| resource.is_internal) == Some(false)
    {
        return Some(VolumeKind::External);
    }
    if resource.and_then(|resource| resource.is_internal) == Some(true) {
        return Some(VolumeKind::Internal);
    }

    let mount_table =
        mount_table.filter(|mount_table| mount_table.status == NativeVolumeStatus::Available);
    if mount_table.and_then(|mount_table| mount_table.is_local) == Some(false) {
        return Some(VolumeKind::Network);
    }

    let native = native?;
    if native.media_removable == Some(true) {
        return Some(VolumeKind::Removable);
    }
    if native.device_internal == Some(false) || native.media_ejectable == Some(true) {
        return Some(VolumeKind::External);
    }
    if native.device_internal == Some(true) {
        return Some(VolumeKind::Internal);
    }
    None
}

fn volume_source(native: Option<&NativeVolumeDescription>) -> String {
    match native {
        Some(native) if native.status == NativeVolumeStatus::Available => {
            let mut fields = vec!["diskarbitration".to_string()];
            if let Some(name) = native.media_bsd_name.as_deref() {
                fields.push(format!("bsd={}", escape_field(name)));
            }
            if let Some(protocol) = native.device_protocol.as_deref() {
                fields.push(format!("protocol={}", escape_field(protocol)));
            }
            fields.join(";")
        }
        Some(native) => format!(
            "filesystem;diskarbitration-{}:{}",
            native.status.as_str(),
            native
                .reason
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "no-reason".to_string())
        ),
        None => "filesystem".to_string(),
    }
}

fn enrich_volume_source(
    source: String,
    resource_status: Option<NativeVolumeStatus>,
    resource: Option<&NativeVolumeResourceValues>,
    mount_table_status: Option<NativeVolumeStatus>,
    mount_table: Option<&NativeVolumeMountTableEntry>,
) -> String {
    let mut source = source;
    if let Some(status) = resource_status {
        source.push_str(";url-resource=");
        source.push_str(status.as_str());
    }
    if let Some(reason) = resource.and_then(|resource| resource.reason.as_deref()) {
        source.push(':');
        source.push_str(&escape_field(reason));
    }
    if let Some(status) = mount_table_status {
        source.push_str(";mount-table=");
        source.push_str(status.as_str());
    }
    if let Some(reason) = mount_table.and_then(|mount_table| mount_table.reason.as_deref()) {
        source.push(':');
        source.push_str(&escape_field(reason));
    }
    source
}

fn stable_identity(
    id: VolumeId,
    path: &Path,
    native: Option<&NativeVolumeDescription>,
    resource: Option<&NativeVolumeResourceValues>,
    mount_table: Option<&NativeVolumeMountTableEntry>,
) -> String {
    if let Some(native) = native.filter(|native| native.status == NativeVolumeStatus::Available) {
        if let Some(uuid) = native
            .volume_uuid
            .as_deref()
            .or(native.media_uuid.as_deref())
        {
            return format!("diskarbitration:uuid:{}", escape_field(uuid));
        }
        if let Some(bsd_name) = native.media_bsd_name.as_deref() {
            return format!("diskarbitration:bsd:{}", escape_field(bsd_name));
        }
    }
    if let Some(resource) =
        resource.filter(|resource| resource.status == NativeVolumeStatus::Available)
    {
        if let Some(uuid) = resource.volume_uuid.as_deref() {
            return format!("url-resource:uuid:{}", escape_field(uuid));
        }
    }
    if let Some(mount_table) =
        mount_table.filter(|mount_table| mount_table.status == NativeVolumeStatus::Available)
    {
        if let (Some(mounted_from), Some(mount_point)) = (
            mount_table.mounted_from.as_deref(),
            mount_table.mount_point.as_deref(),
        ) {
            return format!(
                "mount-table:{}:{}",
                escape_field(mounted_from),
                escape_field(&mount_point.display().to_string())
            );
        }
    }
    format!("dev:{}:{}", id.0, escape_field(&path.display().to_string()))
}

fn mounted_volume_paths() -> Vec<PathBuf> {
    let table = gfm_mac_sys::copy_volume_mount_table();
    if table.status != NativeVolumeStatus::Available {
        return Vec::new();
    }
    let mut paths = table
        .entries
        .into_iter()
        .filter_map(|entry| entry.mount_point)
        .filter(|path| finder_visible_mount_path(path))
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

fn containing_mounted_volume_paths(path: &Path) -> Vec<PathBuf> {
    let mut paths = mounted_volume_paths()
        .into_iter()
        .filter(|root| path.starts_with(root))
        .collect::<Vec<_>>();
    if paths.is_empty() && !path.exists() {
        paths = path
            .ancestors()
            .find(|ancestor| ancestor.exists())
            .map(mounted_volume_paths_for_existing_path)
            .unwrap_or_default();
    }
    paths
}

fn mounted_volume_paths_for_existing_path(path: &Path) -> Vec<PathBuf> {
    mounted_volume_paths()
        .into_iter()
        .filter(|root| path.starts_with(root))
        .collect()
}

fn fallback_volume_paths() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from("/")];
    if let Ok(entries) = fs::read_dir("/Volumes") {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                paths.push(path);
            }
        }
    }
    paths
}

fn finder_visible_mount_path(path: &Path) -> bool {
    path == Path::new("/") || path.starts_with("/Volumes")
}

fn marker_ancestor_volume_paths(path: &Path) -> Vec<PathBuf> {
    path.ancestors()
        .filter(|ancestor| marker_kind(ancestor).is_some())
        .map(PathBuf::from)
        .collect()
}

fn command_policy(
    kind: VolumeKind,
    mount_state: MountState,
    ejectable: bool,
) -> VolumeCommandPolicy {
    if mount_state != MountState::Mounted {
        return VolumeCommandPolicy {
            eject: VolumeCommandState::Disabled,
            mount: VolumeCommandState::Enabled,
            unmount: VolumeCommandState::Hidden,
            reason: Some("volume-not-mounted".to_string()),
        };
    }
    if ejectable {
        VolumeCommandPolicy {
            eject: VolumeCommandState::Enabled,
            mount: VolumeCommandState::Hidden,
            unmount: VolumeCommandState::Enabled,
            reason: None,
        }
    } else {
        VolumeCommandPolicy {
            eject: VolumeCommandState::Hidden,
            mount: VolumeCommandState::Hidden,
            unmount: VolumeCommandState::Disabled,
            reason: Some(format!("{}-volume-not-ejectable", kind.as_str())),
        }
    }
}

fn marker_kind(path: &Path) -> Option<String> {
    let value = fs::read_to_string(path.join(VOLUME_MARKER)).ok()?;
    let value = value.lines().next()?.trim().to_ascii_lowercase();
    (!value.is_empty()).then_some(value)
}

fn marker_removable(marker: Option<&str>) -> Option<bool> {
    match marker {
        Some("external-removable") | Some("removable") | Some("disk-image") => Some(true),
        Some("network")
        | Some("network-smb")
        | Some("network-afp")
        | Some("network-nfs")
        | Some("network-unreachable")
        | Some("network-offline")
        | Some("system")
        | Some("internal") => Some(false),
        _ => None,
    }
}

fn marker_network(marker: Option<&str>) -> Option<bool> {
    match marker {
        Some("network")
        | Some("network-smb")
        | Some("network-afp")
        | Some("network-nfs")
        | Some("network-unreachable")
        | Some("network-offline") => Some(true),
        Some("external")
        | Some("external-removable")
        | Some("removable")
        | Some("disk-image")
        | Some("system")
        | Some("internal") => Some(false),
        _ => None,
    }
}

fn marker_ejectable(marker: Option<&str>) -> Option<bool> {
    match marker {
        Some("external")
        | Some("external-removable")
        | Some("removable")
        | Some("disk-image")
        | Some("network")
        | Some("network-smb")
        | Some("network-afp")
        | Some("network-nfs")
        | Some("network-unreachable")
        | Some("network-offline") => Some(true),
        Some("system") | Some("internal") => Some(false),
        _ => None,
    }
}

fn marker_stable_identity(marker: &str, id: VolumeId, path: &Path) -> String {
    format!(
        "fixture-marker:{}:dev:{}:{}",
        escape_field(marker),
        id.0,
        escape_field(&path.display().to_string())
    )
}

fn volume_label(path: &Path) -> String {
    if path == Path::new("/") {
        "Macintosh HD".to_string()
    } else {
        path.file_name()
            .and_then(|label| label.to_str())
            .filter(|label| !label.is_empty())
            .unwrap_or("Volume")
            .to_string()
    }
}

#[cfg(unix)]
fn volume_id(metadata: &fs::Metadata) -> VolumeId {
    VolumeId(metadata.dev())
}

#[cfg(not(unix))]
fn volume_id(metadata: &fs::Metadata) -> VolumeId {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    metadata.len().hash(&mut hasher);
    metadata.modified().ok().hash(&mut hasher);
    VolumeId(hasher.finish())
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
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn classifies_system_root_as_not_ejectable() {
        let descriptor = VolumeDescriptor::for_path("/").unwrap();

        assert_eq!(descriptor.kind, VolumeKind::System);
        assert!(!descriptor.ejectable);
        assert_eq!(
            descriptor.native_status,
            Some(NativeVolumeStatus::Available)
        );
        assert_eq!(
            descriptor.resource_status,
            Some(NativeVolumeStatus::Available)
        );
        assert_eq!(
            descriptor.mount_table_status,
            Some(NativeVolumeStatus::Available)
        );
        assert_eq!(descriptor.read_only, !descriptor.writable);
        assert_eq!(descriptor.reachable, Some(true));
        assert!(descriptor.stable_identity.starts_with("diskarbitration:"));
        assert_eq!(descriptor.commands.eject, VolumeCommandState::Hidden);
        assert!(descriptor.capacity.total_bytes > 0);
        assert!(descriptor.as_tsv().contains("\tnative-status=available\t"));
        assert!(descriptor.as_tsv().contains("\tstable-id="));
        assert!(descriptor.as_tsv().contains("\tread-only="));
        assert!(descriptor.as_tsv().contains("\treachable=true\t"));
        assert!(descriptor.as_tsv().contains("\tcase-sensitive="));
        assert!(descriptor.as_tsv().contains("\tlocal="));
        assert!(descriptor.as_tsv().contains("\tresource-status=available"));
        assert!(descriptor.as_tsv().contains("\tresource-uuid="));
        assert!(descriptor.as_tsv().contains("\tresource-automounted="));
        assert!(descriptor.as_tsv().contains("\tresource-browsable="));
        assert!(descriptor.as_tsv().contains("\tresource-reachable=true\t"));
        assert!(descriptor.as_tsv().contains("\tresource-remount-url="));
        assert!(descriptor.as_tsv().contains("\tmount-status=available\t"));
        assert!(descriptor.as_tsv().contains("\tvolume-type="));
        assert!(descriptor.as_tsv().contains("\tmedia-kind="));
        assert!(descriptor.as_tsv().contains("\tmedia-name="));
        assert!(descriptor.as_tsv().contains("\tmedia-path="));
        assert!(descriptor.as_tsv().contains("\tmedia-type="));
        assert!(descriptor.as_tsv().contains("\tmedia-leaf="));
        assert!(descriptor.as_tsv().contains("\tmedia-whole="));
        assert!(descriptor.as_tsv().contains("\tmedia-encrypted="));
        assert!(descriptor.as_tsv().contains("\tmedia-block-size="));
        assert!(descriptor.as_tsv().contains("\tmedia-size="));
        assert!(descriptor.as_tsv().contains("\tdevice-path="));
        assert!(descriptor.source.contains("mount-table=available"));
        assert!(descriptor.source.contains("url-resource=available"));
    }

    #[test]
    fn classifies_external_marker_as_ejectable() {
        let root = unique_temp_dir("gfm-volume-external");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();

        let descriptor = VolumeDescriptor::for_path(&root).unwrap();

        assert_eq!(descriptor.kind, VolumeKind::External);
        assert!(descriptor.removable);
        assert_eq!(descriptor.reachable, Some(true));
        assert!(descriptor.ejectable);
        assert!(descriptor.native_status.is_some());
        assert!(descriptor.resource_status.is_some());
        assert!(descriptor.mount_table_status.is_some());
        assert!(descriptor
            .stable_identity
            .starts_with("fixture-marker:external-removable:dev:"));
        assert_eq!(descriptor.commands.eject, VolumeCommandState::Enabled);
        assert!(descriptor
            .as_tsv()
            .contains("source=fixture-marker:external-removable"));
        assert!(descriptor.as_tsv().contains("\tnative-status="));
        assert!(descriptor.as_tsv().contains("\tresource-status="));
        assert!(descriptor.as_tsv().contains("\tmount-status="));
        assert!(descriptor.source.contains("url-resource="));
        assert!(descriptor.source.contains("mount-table="));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn classify_native_volume_uses_url_resource_locality() {
        let resource = resource_values(|values| {
            values.is_local = Some(false);
        });

        let kind = classify_native_volume(
            Path::new("/Volumes/Team Share"),
            None,
            Some(&resource),
            None,
        );

        assert_eq!(kind, Some(VolumeKind::Network));
    }

    #[test]
    fn classify_native_volume_uses_mount_table_locality() {
        let mount_table = mount_table_entry(|entry| {
            entry.is_local = Some(false);
        });

        let kind = classify_native_volume(
            Path::new("/Volumes/Team Share"),
            None,
            None,
            Some(&mount_table),
        );

        assert_eq!(kind, Some(VolumeKind::Network));
    }

    #[test]
    fn classify_native_volume_preserves_disk_image_identity_before_ejectability() {
        let native = native_description(|description| {
            description.volume_kind = Some("disk image".to_string());
        });
        let resource = resource_values(|values| {
            values.is_ejectable = Some(true);
        });

        let kind = classify_native_volume(
            Path::new("/Volumes/Installer"),
            Some(&native),
            Some(&resource),
            None,
        );

        assert_eq!(kind, Some(VolumeKind::DiskImage));
    }

    #[test]
    fn classifies_network_marker_as_network_ejectable() {
        let root = unique_temp_dir("gfm-volume-network");
        fs::write(root.join(VOLUME_MARKER), "network-smb\n").unwrap();

        let descriptor = VolumeDescriptor::for_path(&root).unwrap();

        assert_eq!(descriptor.kind, VolumeKind::Network);
        assert!(descriptor.network);
        assert!(descriptor.ejectable);
        assert_eq!(descriptor.commands.unmount, VolumeCommandState::Enabled);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn containing_path_report_prefers_marker_volume_ancestor() {
        let root = unique_temp_dir("gfm-volume-containing");
        let nested = root.join("Project").join("Preview.pdf");
        fs::create_dir_all(nested.parent().unwrap()).unwrap();
        fs::write(root.join(VOLUME_MARKER), "network-smb\n").unwrap();
        fs::write(&nested, "%PDF-1.7\n").unwrap();

        let report = VolumeDiscoveryReport::for_containing_path(&nested);
        let volume = report.volume_for_path(&nested).unwrap();

        assert_eq!(volume.path, root);
        assert_eq!(volume.kind, VolumeKind::Network);

        fs::remove_dir_all(volume.path.clone()).unwrap();
    }

    #[test]
    fn network_unreachable_marker_reports_offline_reachability() {
        let root = unique_temp_dir("gfm-volume-network-offline");
        fs::write(root.join(VOLUME_MARKER), "network-unreachable\n").unwrap();

        let descriptor = VolumeDescriptor::for_path(&root).unwrap();

        assert_eq!(descriptor.kind, VolumeKind::Network);
        assert!(descriptor.network);
        assert_eq!(descriptor.reachable, Some(false));
        assert!(descriptor.as_tsv().contains("\treachable=false\t"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn network_reachability_prefers_native_reachability_over_existing_path() {
        let root = unique_temp_dir("gfm-volume-network-unreachable");
        let resource = resource_values(|values| {
            values.is_reachable = Some(false);
            values.is_browsable = Some(true);
        });

        let reachable = volume_reachability(true, MountState::Mounted, Some(&resource), &root);

        assert_eq!(reachable, Some(false));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn volume_operation_refuses_system_root() {
        let report = VolumeOperationReport::execute("/", VolumeOperation::Unmount).unwrap();

        assert_eq!(report.operation, VolumeOperation::Unmount);
        assert_eq!(report.disposition, VolumeOperationDisposition::Refused);
        assert_eq!(report.native_status, None);
        assert_eq!(report.dissenter_status, None);
        assert_eq!(report.reason, "system-volume-operation-refused");
        assert!(report.as_tsv().contains("\tdisposition=refused\t"));
        assert!(report.as_tsv().contains("\tdissenter-status=-\t"));
    }

    #[test]
    fn volume_operation_refuses_fixture_volume_before_native_call() {
        let root = unique_temp_dir("gfm-volume-operation-fixture");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();

        let report = VolumeOperationReport::execute(&root, VolumeOperation::Eject).unwrap();

        assert_eq!(report.disposition, VolumeOperationDisposition::Refused);
        assert_eq!(report.native_status, None);
        assert_eq!(report.dissenter_status, None);
        assert_eq!(report.reason, "fixture-volume-native-operation-disabled");
        assert!(report.as_tsv().contains("\tdissenter-status=-\t"));
        assert!(report
            .as_tsv()
            .contains("\tvolume-kind=external\tmount=mounted\t"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn volume_mount_operation_is_typed_unsupported() {
        let root = unique_temp_dir("gfm-volume-operation-mount");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();

        let report = VolumeOperationReport::execute(&root, VolumeOperation::Mount).unwrap();

        assert_eq!(report.disposition, VolumeOperationDisposition::Unsupported);
        assert_eq!(report.native_status, None);
        assert_eq!(
            report.reason,
            "native-mount-requires-unmounted-disk-identity"
        );
        assert!(report.as_tsv().starts_with("volume-operation\tmount\t"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn volume_mount_identity_refuses_invalid_bsd_name_before_native_call() {
        let report = VolumeMountIdentityReport::execute("not/a/disk");

        assert_eq!(report.bsd_name, "not/a/disk");
        assert_eq!(report.disposition, VolumeOperationDisposition::Unsupported);
        assert_eq!(
            report.native_status,
            NativeVolumeOperationStatus::Unsupported
        );
        assert_eq!(report.dissenter_status, None);
        assert_eq!(report.reason, "diskarbitration-mount-requires-bsd-name");
        assert!(report
            .as_tsv()
            .starts_with("volume-mount-bsd\tbsd-name=not/a/disk\t"));
    }

    #[test]
    fn volume_mount_identity_refuses_malformed_bsd_name_before_native_call() {
        let report = VolumeMountIdentityReport::execute("notadisk");

        assert_eq!(report.bsd_name, "notadisk");
        assert_eq!(report.disposition, VolumeOperationDisposition::Unsupported);
        assert_eq!(
            report.native_status,
            NativeVolumeOperationStatus::Unsupported
        );
        assert_eq!(report.dissenter_status, None);
        assert_eq!(report.reason, "diskarbitration-mount-requires-bsd-name");
        assert!(report
            .as_tsv()
            .starts_with("volume-mount-bsd\tbsd-name=notadisk\t"));
    }

    #[test]
    fn discovery_report_orders_volumes_stably() {
        let first = unique_temp_dir("gfm-volume-a");
        let second = unique_temp_dir("gfm-volume-b");
        fs::write(first.join(VOLUME_MARKER), "network-smb\n").unwrap();
        fs::write(second.join(VOLUME_MARKER), "external-removable\n").unwrap();

        let report = VolumeDiscoveryReport::from_paths(vec![second.clone(), first.clone()]);
        let tsv = report.as_tsv();

        assert!(tsv.starts_with("volumes\tcount=2"));
        assert!(
            tsv.find(first.to_str().unwrap()).unwrap()
                < tsv.find(second.to_str().unwrap()).unwrap()
        );

        fs::remove_dir_all(first).unwrap();
        fs::remove_dir_all(second).unwrap();
    }

    #[test]
    fn topology_diff_reports_connected_and_disconnected_volumes() {
        let first = unique_temp_dir("gfm-volume-topology-first");
        let second = unique_temp_dir("gfm-volume-topology-second");
        fs::write(first.join(VOLUME_MARKER), "external-removable\n").unwrap();
        fs::write(second.join(VOLUME_MARKER), "network-smb\n").unwrap();
        let previous = VolumeDiscoveryReport::from_paths(vec![first.clone()]);
        let current = VolumeDiscoveryReport::from_paths(vec![second.clone()]);

        let diff = VolumeTopologyDiff::evaluate(&previous, &current);

        assert_eq!(diff.changes.len(), 2);
        assert!(diff.changes.iter().any(|change| change.kind
            == VolumeTopologyChangeKind::Disconnected
            && change.reason == "volume-disconnected"
            && change.invalidate_index_admission));
        assert!(diff
            .changes
            .iter()
            .any(|change| change.kind == VolumeTopologyChangeKind::Connected
                && change.reason == "volume-connected"
                && change.rescan_index));
        assert!(diff.as_tsv().starts_with("volume-topology-diff\tcount=2"));

        fs::remove_dir_all(first).unwrap();
        fs::remove_dir_all(second).unwrap();
    }

    #[test]
    fn topology_diff_keeps_label_changes_out_of_index_rescan() {
        let root = unique_temp_dir("gfm-volume-topology-label");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();
        let previous_volume = VolumeDescriptor::for_path(&root).unwrap();
        let mut current_volume = previous_volume.clone();
        current_volume.label = "Renamed Drive".to_string();
        let previous = VolumeDiscoveryReport {
            volumes: vec![previous_volume],
        };
        let current = VolumeDiscoveryReport {
            volumes: vec![current_volume],
        };

        let diff = VolumeTopologyDiff::evaluate(&previous, &current);

        assert_eq!(diff.changes.len(), 1);
        assert_eq!(diff.changes[0].reason, "volume-label-changed");
        assert!(diff.changes[0].invalidate_sidebar);
        assert!(!diff.changes[0].invalidate_operation_policy);
        assert!(!diff.changes[0].invalidate_index_admission);
        assert!(!diff.changes[0].rescan_index);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn topology_diff_invalidates_policy_for_native_identity_changes() {
        let root = unique_temp_dir("gfm-volume-topology-identity");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();
        let previous_volume = VolumeDescriptor::for_path(&root).unwrap();
        let mut current_volume = previous_volume.clone();
        current_volume.volume_uuid = Some("APFS-VOLUME-UUID".to_string());
        current_volume.media_uuid = Some("APFS-CONTAINER-UUID".to_string());
        current_volume.media_content = Some("Apple_APFS".to_string());
        current_volume.bsd_name = Some("disk4s1".to_string());
        current_volume.mount_from = Some("/dev/disk4s1".to_string());
        current_volume.media_name = Some("Container disk4".to_string());
        current_volume.media_path = Some("IODeviceTree:/PCI0@0/AppleAPFSMedia".to_string());
        let previous = VolumeDiscoveryReport {
            volumes: vec![previous_volume],
        };
        let current = VolumeDiscoveryReport {
            volumes: vec![current_volume],
        };

        let diff = VolumeTopologyDiff::evaluate(&previous, &current);

        assert_eq!(diff.changes.len(), 1);
        assert_eq!(diff.changes[0].reason, "volume-identity-changed");
        assert!(diff.changes[0].invalidate_sidebar);
        assert!(diff.changes[0].invalidate_operation_policy);
        assert!(diff.changes[0].invalidate_index_admission);
        assert!(diff.changes[0].rescan_index);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn topology_diff_invalidates_policy_for_filesystem_trait_changes() {
        let root = unique_temp_dir("gfm-volume-topology-filesystem");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();
        let previous_volume = VolumeDescriptor::for_path(&root).unwrap();
        let mut current_volume = previous_volume.clone();
        current_volume.filesystem = Some("apfs".to_string());
        current_volume.mount_filesystem = Some("apfs".to_string());
        current_volume.mount_flags = Some(0x0000_1000);
        current_volume.mount_local = Some(true);
        current_volume.case_sensitive = Some(true);
        current_volume.case_preserving = Some(true);
        current_volume.resource_reachable = Some(true);
        current_volume.device_protocol = Some("USB".to_string());
        current_volume.device_model = Some("External SSD".to_string());
        current_volume.device_vendor = Some("Samsung".to_string());
        current_volume.volume_type = Some("apfs".to_string());
        current_volume.media_kind = Some("IOMedia".to_string());
        current_volume.media_type = Some("Generic".to_string());
        current_volume.media_leaf = Some(true);
        current_volume.media_whole = Some(false);
        current_volume.media_encrypted = Some(true);
        current_volume.media_block_size_bytes = Some(4096);
        current_volume.media_size_bytes = Some(1024 * 1024 * 1024);
        current_volume.device_path = Some("IODeviceTree:/PCI0".to_string());
        current_volume.internal = Some(false);
        let previous = VolumeDiscoveryReport {
            volumes: vec![previous_volume],
        };
        let current = VolumeDiscoveryReport {
            volumes: vec![current_volume],
        };

        let diff = VolumeTopologyDiff::evaluate(&previous, &current);

        assert_eq!(diff.changes.len(), 1);
        assert_eq!(diff.changes[0].reason, "volume-filesystem-changed");
        assert!(diff.changes[0].invalidate_sidebar);
        assert!(diff.changes[0].invalidate_operation_policy);
        assert!(diff.changes[0].invalidate_index_admission);
        assert!(diff.changes[0].rescan_index);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mounted_volume_paths_include_system_root_from_mount_table() {
        let paths = mounted_volume_paths();

        assert!(paths.iter().any(|path| path == Path::new("/")));
        assert!(paths.iter().all(|path| finder_visible_mount_path(path)));
    }

    #[test]
    fn volume_event_stream_exposes_owned_diskarbitration_lifecycle() {
        let stream = VolumeEventStream::start();

        assert!(stream.is_attached() || stream.try_recv().is_some());
        drop(stream);
    }

    #[test]
    fn volume_event_invalidation_updates_sidebar_policy_and_index_admission() {
        let root = unique_temp_dir("gfm-volume-event-invalidation");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();
        let descriptor = VolumeDescriptor::for_path(&root).unwrap();

        let report = VolumeEventInvalidationReport::from_parts(
            VolumeEventKind::DescriptionChanged,
            NativeVolumeStatus::Available,
            Some(root.clone()),
            Some(&descriptor),
            None,
        );

        assert_eq!(report.current_kind, Some(VolumeKind::External));
        assert_eq!(report.current_mount_state, Some(MountState::Mounted));
        assert_eq!(report.previous_kind, None);
        assert_eq!(report.previous_mount_state, None);
        assert!(report.invalidate_sidebar);
        assert!(report.invalidate_operation_policy);
        assert!(report.invalidate_index_admission);
        assert!(report.rescan_index);
        assert!(report
            .as_tsv()
            .starts_with("volume-event-invalidation\tkind=description-changed\t"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn disappeared_volume_event_reports_previous_volume_and_unmounted_current_state() {
        let root = unique_temp_dir("gfm-volume-event-disappeared");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();
        let descriptor = VolumeDescriptor::for_path(&root).unwrap();

        let report = VolumeEventInvalidationReport::from_parts(
            VolumeEventKind::Disappeared,
            NativeVolumeStatus::Available,
            Some(root.clone()),
            Some(&descriptor),
            None,
        );

        assert_eq!(report.previous_kind, Some(VolumeKind::External));
        assert_eq!(report.previous_mount_state, Some(MountState::Mounted));
        assert_eq!(report.current_kind, None);
        assert_eq!(report.current_mount_state, Some(MountState::Unmounted));
        assert!(report.invalidate_sidebar);
        assert!(report.invalidate_operation_policy);
        assert!(report.invalidate_index_admission);
        assert!(report.rescan_index);
        assert!(report.as_tsv().contains(
            "\tprevious-kind=external\tprevious-mount=mounted\tcurrent-kind=-\tcurrent-mount=unmounted\t"
        ));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn topology_diff_invalidates_policy_for_reachability_changes() {
        let root = unique_temp_dir("gfm-volume-topology-reachability");
        fs::write(root.join(VOLUME_MARKER), "network-smb\n").unwrap();
        let previous_volume = VolumeDescriptor::for_path(&root).unwrap();
        let mut current_volume = previous_volume.clone();
        current_volume.reachable = Some(false);
        let previous = VolumeDiscoveryReport {
            volumes: vec![previous_volume],
        };
        let current = VolumeDiscoveryReport {
            volumes: vec![current_volume],
        };

        let diff = VolumeTopologyDiff::evaluate(&previous, &current);

        assert_eq!(diff.changes.len(), 1);
        assert_eq!(diff.changes[0].reason, "volume-locality-changed");
        assert!(diff.changes[0].invalidate_sidebar);
        assert!(diff.changes[0].invalidate_operation_policy);
        assert!(diff.changes[0].invalidate_index_admission);
        assert!(diff.changes[0].rescan_index);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unavailable_volume_event_still_invalidates_downstream_state() {
        let report = VolumeEventInvalidationReport::from_parts(
            VolumeEventKind::Unavailable,
            NativeVolumeStatus::Unavailable,
            None,
            None,
            Some("diskarbitration-session-unavailable".to_string()),
        );

        assert_eq!(report.current_kind, None);
        assert!(report.invalidate_sidebar);
        assert!(report.invalidate_operation_policy);
        assert!(report.invalidate_index_admission);
        assert!(report.rescan_index);
        assert!(report
            .as_tsv()
            .contains("\treason=diskarbitration-session-unavailable"));
    }

    #[test]
    fn stable_identity_uses_url_resource_uuid_before_mount_table() {
        let resource = resource_values(|values| {
            values.volume_uuid = Some("RESOURCE-UUID".to_string());
        });
        let mount_table = mount_table_entry(|entry| {
            entry.mounted_from = Some("/dev/disk9s9".to_string());
            entry.mount_point = Some(PathBuf::from("/Volumes/Remote"));
        });

        let identity = stable_identity(
            VolumeId(42),
            Path::new("/Volumes/Remote"),
            None,
            Some(&resource),
            Some(&mount_table),
        );

        assert_eq!(identity, "url-resource:uuid:RESOURCE-UUID");
    }

    #[test]
    fn volume_operation_refuses_missing_path_without_native_submission() {
        let report = VolumeOperationReport::execute(
            "/tmp/gfm-volume-operation-missing",
            VolumeOperation::Eject,
        )
        .unwrap();

        assert_eq!(report.disposition, VolumeOperationDisposition::Refused);
        assert_eq!(report.native_status, None);
        assert_eq!(report.reason, "volume-path-missing");
        assert!(report.as_tsv().contains("\tdisposition=refused\t"));
    }

    #[test]
    fn native_volume_operation_status_maps_to_typed_dispositions() {
        assert_eq!(
            disposition_for_native_operation(NativeVolumeOperationStatus::Succeeded),
            VolumeOperationDisposition::Completed
        );
        assert_eq!(
            disposition_for_native_operation(NativeVolumeOperationStatus::Busy),
            VolumeOperationDisposition::Busy
        );
        assert_eq!(
            disposition_for_native_operation(NativeVolumeOperationStatus::NotPermitted),
            VolumeOperationDisposition::Denied
        );
        assert_eq!(
            disposition_for_native_operation(NativeVolumeOperationStatus::NotPrivileged),
            VolumeOperationDisposition::Denied
        );
        assert_eq!(
            disposition_for_native_operation(NativeVolumeOperationStatus::Unsupported),
            VolumeOperationDisposition::Unsupported
        );
        assert_eq!(
            disposition_for_native_operation(NativeVolumeOperationStatus::NotMounted),
            VolumeOperationDisposition::Refused
        );
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "{}-{}-{}",
            prefix,
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn resource_values(
        configure: impl FnOnce(&mut NativeVolumeResourceValues),
    ) -> NativeVolumeResourceValues {
        let mut values = NativeVolumeResourceValues {
            status: NativeVolumeStatus::Available,
            is_automounted: None,
            is_browsable: None,
            is_ejectable: None,
            is_internal: None,
            is_local: None,
            is_read_only: None,
            is_reachable: None,
            is_removable: None,
            remount_url: None,
            supports_case_preserved_names: None,
            supports_case_sensitive_names: None,
            volume_uuid: None,
            reason: None,
        };
        configure(&mut values);
        values
    }

    fn mount_table_entry(
        configure: impl FnOnce(&mut NativeVolumeMountTableEntry),
    ) -> NativeVolumeMountTableEntry {
        let mut entry = NativeVolumeMountTableEntry {
            status: NativeVolumeStatus::Available,
            mount_point: None,
            mounted_from: None,
            filesystem_type: None,
            flags: None,
            is_read_only: None,
            is_local: None,
            reason: None,
        };
        configure(&mut entry);
        entry
    }

    fn native_description(
        configure: impl FnOnce(&mut NativeVolumeDescription),
    ) -> NativeVolumeDescription {
        let mut description = NativeVolumeDescription {
            status: NativeVolumeStatus::Available,
            volume_name: None,
            volume_kind: None,
            volume_mountable: None,
            volume_type: None,
            volume_uuid: None,
            volume_path: None,
            volume_network: None,
            media_bsd_name: None,
            media_bsd_major: None,
            media_bsd_minor: None,
            media_bsd_unit: None,
            media_content: None,
            media_kind: None,
            media_leaf: None,
            media_name: None,
            media_path: None,
            media_removable: None,
            media_ejectable: None,
            media_writable: None,
            media_type: None,
            media_uuid: None,
            media_whole: None,
            media_encrypted: None,
            media_block_size_bytes: None,
            media_size_bytes: None,
            device_internal: None,
            device_model: None,
            device_path: None,
            device_protocol: None,
            device_vendor: None,
            reason: None,
        };
        configure(&mut description);
        description
    }
}
