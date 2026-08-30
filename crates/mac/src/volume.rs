use gfm_mac_sys::{
    NativeVolumeDescription, NativeVolumeMountTableEntry, NativeVolumeOperation,
    NativeVolumeOperationStatus, NativeVolumeResourceValues, NativeVolumeStatus,
};
use gfm_types::{GfmError, Result, VolumeId};
use std::collections::{BTreeMap, BTreeSet};
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
pub enum ApfsVolumeRole {
    System,
    Data,
    Preboot,
    Recovery,
    Vm,
    Update,
    Xart,
    Hardware,
    Backup,
    Unknown,
}

impl ApfsVolumeRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Data => "data",
            Self::Preboot => "preboot",
            Self::Recovery => "recovery",
            Self::Vm => "vm",
            Self::Update => "update",
            Self::Xart => "xart",
            Self::Hardware => "hardware",
            Self::Backup => "backup",
            Self::Unknown => "unknown",
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
    pub resource_encrypted: Option<bool>,
    pub resource_reachable: Option<bool>,
    pub resource_root_file_system: Option<bool>,
    pub resource_supports_file_cloning: Option<bool>,
    pub resource_supports_hard_links: Option<bool>,
    pub resource_supports_sparse_files: Option<bool>,
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
    pub apfs_container_uuid: Option<String>,
    pub apfs_role: Option<ApfsVolumeRole>,
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
    pub fn platform_state_unavailable(&self) -> bool {
        self.native_status == Some(NativeVolumeStatus::Unavailable)
            && self.resource_status == Some(NativeVolumeStatus::Unavailable)
            && self.mount_table_status == Some(NativeVolumeStatus::Unavailable)
    }

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
        let mount_state = MountState::Mounted;
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
        let local = volume_local_state(
            marker_value,
            native.as_ref(),
            resource.as_ref(),
            mount_table.as_ref(),
            kind,
        );
        let network = volume_network_state(
            marker_value,
            native.as_ref(),
            resource.as_ref(),
            mount_table.as_ref(),
            kind,
        );
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
        let access = volume_access_state(
            marker_value,
            native.as_ref(),
            resource.as_ref(),
            mount_table.as_ref(),
            !metadata.permissions().readonly(),
        );
        let writable = access.writable;
        let read_only = access.read_only;
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
        let apfs_container_uuid = apfs_container_uuid(native.as_ref(), mount_table.as_ref());
        let apfs_role = apfs_volume_role(native.as_ref());
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
            resource_encrypted: resource.as_ref().and_then(|resource| resource.is_encrypted),
            resource_reachable: resource.as_ref().and_then(|resource| resource.is_reachable),
            resource_root_file_system: resource
                .as_ref()
                .and_then(|resource| resource.is_root_file_system),
            resource_supports_file_cloning: resource
                .as_ref()
                .and_then(|resource| resource.supports_file_cloning),
            resource_supports_hard_links: resource
                .as_ref()
                .and_then(|resource| resource.supports_hard_links),
            resource_supports_sparse_files: resource
                .as_ref()
                .and_then(|resource| resource.supports_sparse_files),
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
            apfs_container_uuid,
            apfs_role,
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
            "volume\t{}\t{}\tpath={}\tkind={}\tmount={}\tremovable={}\tnetwork={}\treachable={}\tejectable={}\ttotal={}\tavailable={}\teject={}\tmount={}\tunmount={}\tsource={}\treason={}\tstable-id={}\tnative-status={}\twritable={}\tread-only={}\tcase-sensitive={}\tcase-preserving={}\tlocal={}\tinternal={}\tmountable={}\tbsd={}\tvolume-uuid={}\tapfs-container-uuid={}\tapfs-role={}\tmedia-uuid={}\tfs={}\tmedia-content={}\tprotocol={}\tmodel={}\tvendor={}\tresource-status={}\tresource-uuid={}\tresource-automounted={}\tresource-browsable={}\tresource-encrypted={}\tresource-reachable={}\tresource-root-filesystem={}\tresource-supports-file-cloning={}\tresource-supports-hard-links={}\tresource-supports-sparse-files={}\tresource-remount-url={}\tmount-status={}\tmount-from={}\tmount-fs={}\tmount-flags={}\tmount-read-only={}\tmount-local={}\tvolume-type={}\tmedia-kind={}\tmedia-name={}\tmedia-path={}\tmedia-type={}\tmedia-leaf={}\tmedia-whole={}\tmedia-encrypted={}\tmedia-block-size={}\tmedia-size={}\tdevice-path={}",
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
            self.apfs_container_uuid
                .as_deref()
                .map(escape_field)
                .unwrap_or_else(|| "-".to_string()),
            self.apfs_role
                .map(ApfsVolumeRole::as_str)
                .unwrap_or("-"),
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
            self.resource_encrypted
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.resource_reachable
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.resource_root_file_system
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.resource_supports_file_cloning
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.resource_supports_hard_links
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.resource_supports_sparse_files
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
        let mut volumes: Vec<_> = unique_volume_paths(paths)
            .into_iter()
            .filter_map(|path| VolumeDescriptor::for_path(path).ok())
            .collect();
        normalize_discovered_volumes(&mut volumes);
        Self { volumes }
    }

    pub fn from_paths_checked(paths: Vec<PathBuf>) -> Result<Self> {
        let paths = unique_volume_paths(paths);
        let mut volumes = Vec::with_capacity(paths.len());
        for path in paths {
            volumes.push(VolumeDescriptor::for_path(path)?);
        }
        normalize_discovered_volumes(&mut volumes);
        Ok(Self { volumes })
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
        let lookup_path = normalized_lookup_path(path);
        let mut best = None;
        let mut best_depth = 0;
        for volume in &self.volumes {
            let Some(depth) = volume_match_depth(volume, path, lookup_path.as_deref()) else {
                continue;
            };
            if depth >= best_depth {
                best = Some(volume);
                best_depth = depth;
            }
        }
        best
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
    pub previous_case_sensitive: Option<bool>,
    pub current_case_sensitive: Option<bool>,
    pub previous_native_status: Option<NativeVolumeStatus>,
    pub current_native_status: Option<NativeVolumeStatus>,
    pub previous_resource_status: Option<NativeVolumeStatus>,
    pub current_resource_status: Option<NativeVolumeStatus>,
    pub previous_mount_table_status: Option<NativeVolumeStatus>,
    pub current_mount_table_status: Option<NativeVolumeStatus>,
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
            previous_case_sensitive: None,
            current_case_sensitive: volume.case_sensitive,
            previous_native_status: None,
            current_native_status: volume.native_status,
            previous_resource_status: None,
            current_resource_status: volume.resource_status,
            previous_mount_table_status: None,
            current_mount_table_status: volume.mount_table_status,
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
            previous_case_sensitive: volume.case_sensitive,
            current_case_sensitive: None,
            previous_native_status: volume.native_status,
            current_native_status: None,
            previous_resource_status: volume.resource_status,
            current_resource_status: None,
            previous_mount_table_status: volume.mount_table_status,
            current_mount_table_status: None,
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
                | "volume-media-truth-changed"
                | "volume-identity-changed"
                | "volume-case-sensitivity-changed"
                | "volume-api-status-changed"
                | "volume-apfs-metadata-changed"
                | "volume-mount-table-changed"
                | "volume-filesystem-changed"
        );
        let rescan_index = matches!(
            reason,
            "volume-path-changed"
                | "mount-state-changed"
                | "volume-kind-changed"
                | "volume-access-changed"
                | "volume-locality-changed"
                | "volume-ejectability-changed"
                | "volume-media-truth-changed"
                | "volume-identity-changed"
                | "volume-case-sensitivity-changed"
                | "volume-api-status-changed"
                | "volume-apfs-metadata-changed"
                | "volume-mount-table-changed"
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
            previous_case_sensitive: previous.case_sensitive,
            current_case_sensitive: current.case_sensitive,
            previous_native_status: previous.native_status,
            current_native_status: current.native_status,
            previous_resource_status: previous.resource_status,
            current_resource_status: current.resource_status,
            previous_mount_table_status: previous.mount_table_status,
            current_mount_table_status: current.mount_table_status,
            invalidate_sidebar: true,
            invalidate_operation_policy: invalidates_policy,
            invalidate_index_admission: invalidates_policy,
            rescan_index,
            reason: reason.to_string(),
        })
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "volume-topology\t{}\tstable-id={}\tlabel={}\tpath={}\tprevious-kind={}\tcurrent-kind={}\tprevious-mount={}\tcurrent-mount={}\tprevious-case-sensitive={}\tcurrent-case-sensitive={}\tprevious-native-status={}\tcurrent-native-status={}\tprevious-resource-status={}\tcurrent-resource-status={}\tprevious-mount-status={}\tcurrent-mount-status={}\tsidebar={}\toperation-policy={}\tindex-admission={}\trescan-index={}\treason={}",
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
            self.previous_case_sensitive
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.current_case_sensitive
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.previous_native_status
                .map(NativeVolumeStatus::as_str)
                .unwrap_or("-"),
            self.current_native_status
                .map(NativeVolumeStatus::as_str)
                .unwrap_or("-"),
            self.previous_resource_status
                .map(NativeVolumeStatus::as_str)
                .unwrap_or("-"),
            self.current_resource_status
                .map(NativeVolumeStatus::as_str)
                .unwrap_or("-"),
            self.previous_mount_table_status
                .map(NativeVolumeStatus::as_str)
                .unwrap_or("-"),
            self.current_mount_table_status
                .map(NativeVolumeStatus::as_str)
                .unwrap_or("-"),
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
        let previous = normalized_volume_map(&previous.volumes);
        let current = normalized_volume_map(&current.volumes);
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
    pub previous_case_sensitive: Option<bool>,
    pub previous_native_status: Option<NativeVolumeStatus>,
    pub previous_resource_status: Option<NativeVolumeStatus>,
    pub previous_mount_table_status: Option<NativeVolumeStatus>,
    pub current_kind: Option<VolumeKind>,
    pub current_mount_state: Option<MountState>,
    pub current_case_sensitive: Option<bool>,
    pub current_native_status: Option<NativeVolumeStatus>,
    pub current_resource_status: Option<NativeVolumeStatus>,
    pub current_mount_table_status: Option<NativeVolumeStatus>,
    pub invalidate_sidebar: bool,
    pub invalidate_operation_policy: bool,
    pub invalidate_index_admission: bool,
    pub rescan_index: bool,
    pub reason: String,
}

pub struct VolumeEventStream {
    stream: gfm_mac_sys::NativeVolumeEventStream,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeEventState {
    report: VolumeDiscoveryReport,
    stable_index: BTreeMap<String, usize>,
    path_index: BTreeMap<PathBuf, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeEventStateTransition {
    pub previous: Option<VolumeDescriptor>,
    pub current: Option<VolumeDescriptor>,
    pub invalidation: VolumeEventInvalidationReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolumeEventShutdownReport {
    pub attached_before_shutdown: bool,
    pub stop_requested: bool,
    pub thread_joined: bool,
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
        let kind = event.kind.into();
        let descriptor = native_event_descriptor(kind, path.as_deref());
        Self {
            kind,
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

fn native_event_descriptor(kind: VolumeEventKind, path: Option<&Path>) -> Option<VolumeDescriptor> {
    if kind == VolumeEventKind::Unavailable {
        return None;
    }
    let path = path?;
    match path.try_exists() {
        Ok(true) => VolumeDescriptor::for_path(path).ok(),
        Ok(false) | Err(_) => None,
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

    pub fn from_transition(
        kind: VolumeEventKind,
        native_status: NativeVolumeStatus,
        previous: Option<&VolumeDescriptor>,
        current: Option<&VolumeDescriptor>,
        native_reason: Option<String>,
    ) -> Self {
        match kind {
            VolumeEventKind::Appeared => Self::from_parts(
                kind,
                native_status,
                current.map(|descriptor| descriptor.path.clone()),
                current,
                native_reason,
            ),
            VolumeEventKind::Disappeared => Self::from_parts(
                kind,
                native_status,
                previous.map(|descriptor| descriptor.path.clone()),
                previous,
                native_reason,
            ),
            VolumeEventKind::Unavailable => {
                let path = current
                    .or(previous)
                    .map(|descriptor| descriptor.path.clone());
                let event_visible =
                    path.is_some() || native_status != NativeVolumeStatus::Available;
                Self {
                    kind,
                    native_status,
                    path,
                    previous_kind: previous.map(|descriptor| descriptor.kind),
                    previous_mount_state: previous.map(|descriptor| descriptor.mount_state),
                    previous_case_sensitive: previous
                        .and_then(|descriptor| descriptor.case_sensitive),
                    previous_native_status: previous
                        .and_then(|descriptor| descriptor.native_status),
                    previous_resource_status: previous
                        .and_then(|descriptor| descriptor.resource_status),
                    previous_mount_table_status: previous
                        .and_then(|descriptor| descriptor.mount_table_status),
                    current_kind: current.map(|descriptor| descriptor.kind),
                    current_mount_state: current.map(|descriptor| descriptor.mount_state),
                    current_case_sensitive: current
                        .and_then(|descriptor| descriptor.case_sensitive),
                    current_native_status: current.and_then(|descriptor| descriptor.native_status),
                    current_resource_status: current
                        .and_then(|descriptor| descriptor.resource_status),
                    current_mount_table_status: current
                        .and_then(|descriptor| descriptor.mount_table_status),
                    invalidate_sidebar: event_visible,
                    invalidate_operation_policy: event_visible,
                    invalidate_index_admission: event_visible,
                    rescan_index: event_visible,
                    reason: native_reason.unwrap_or_else(|| "volume-event-unavailable".to_string()),
                }
            }
            VolumeEventKind::DescriptionChanged => {
                let path = current
                    .or(previous)
                    .map(|descriptor| descriptor.path.clone());
                let current_kind = current.map(|descriptor| descriptor.kind);
                let current_mount_state = current.map(|descriptor| descriptor.mount_state);
                let previous_kind = previous.map(|descriptor| descriptor.kind);
                let previous_mount_state = previous.map(|descriptor| descriptor.mount_state);
                let topology_reason = previous
                    .zip(current)
                    .and_then(|(previous, current)| topology_change_reason(previous, current))
                    .map(str::to_string);
                let reason = topology_reason
                    .or_else(|| {
                        if previous.is_some() && current.is_some() {
                            None
                        } else {
                            native_reason
                        }
                    })
                    .unwrap_or_else(|| "volume-event-description-changed".to_string());
                let heavy = description_change_invalidates_policy(&reason);
                let visible =
                    path.is_some() || native_status != NativeVolumeStatus::Available || heavy;
                Self {
                    kind,
                    native_status,
                    path,
                    previous_kind,
                    previous_mount_state,
                    previous_case_sensitive: previous
                        .and_then(|descriptor| descriptor.case_sensitive),
                    previous_native_status: previous
                        .and_then(|descriptor| descriptor.native_status),
                    previous_resource_status: previous
                        .and_then(|descriptor| descriptor.resource_status),
                    previous_mount_table_status: previous
                        .and_then(|descriptor| descriptor.mount_table_status),
                    current_kind,
                    current_mount_state,
                    current_case_sensitive: current
                        .and_then(|descriptor| descriptor.case_sensitive),
                    current_native_status: current.and_then(|descriptor| descriptor.native_status),
                    current_resource_status: current
                        .and_then(|descriptor| descriptor.resource_status),
                    current_mount_table_status: current
                        .and_then(|descriptor| descriptor.mount_table_status),
                    invalidate_sidebar: visible,
                    invalidate_operation_policy: visible && heavy,
                    invalidate_index_admission: visible && heavy,
                    rescan_index: visible && heavy,
                    reason,
                }
            }
        }
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
            VolumeEventKind::DescriptionChanged => native_reason
                .as_deref()
                .unwrap_or("volume-event-description-changed"),
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
            previous_case_sensitive: (kind == VolumeEventKind::Disappeared)
                .then(|| descriptor.and_then(|descriptor| descriptor.case_sensitive))
                .flatten(),
            previous_native_status: (kind == VolumeEventKind::Disappeared)
                .then(|| descriptor.and_then(|descriptor| descriptor.native_status))
                .flatten(),
            previous_resource_status: (kind == VolumeEventKind::Disappeared)
                .then(|| descriptor.and_then(|descriptor| descriptor.resource_status))
                .flatten(),
            previous_mount_table_status: (kind == VolumeEventKind::Disappeared)
                .then(|| descriptor.and_then(|descriptor| descriptor.mount_table_status))
                .flatten(),
            current_kind: (kind != VolumeEventKind::Disappeared)
                .then(|| descriptor.map(|descriptor| descriptor.kind))
                .flatten(),
            current_mount_state: if kind == VolumeEventKind::Disappeared && event_visible {
                Some(MountState::Unmounted)
            } else {
                descriptor.map(|descriptor| descriptor.mount_state)
            },
            current_case_sensitive: (kind != VolumeEventKind::Disappeared)
                .then(|| descriptor.and_then(|descriptor| descriptor.case_sensitive))
                .flatten(),
            current_native_status: (kind != VolumeEventKind::Disappeared)
                .then(|| descriptor.and_then(|descriptor| descriptor.native_status))
                .flatten(),
            current_resource_status: (kind != VolumeEventKind::Disappeared)
                .then(|| descriptor.and_then(|descriptor| descriptor.resource_status))
                .flatten(),
            current_mount_table_status: (kind != VolumeEventKind::Disappeared)
                .then(|| descriptor.and_then(|descriptor| descriptor.mount_table_status))
                .flatten(),
            invalidate_sidebar: invalidates,
            invalidate_operation_policy: invalidates,
            invalidate_index_admission: invalidates,
            rescan_index: invalidates,
            reason: reason.to_string(),
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "volume-event-invalidation\tkind={}\tnative-status={}\tpath={}\tprevious-kind={}\tprevious-mount={}\tprevious-case-sensitive={}\tprevious-native-status={}\tprevious-resource-status={}\tprevious-mount-status={}\tcurrent-kind={}\tcurrent-mount={}\tcurrent-case-sensitive={}\tcurrent-native-status={}\tcurrent-resource-status={}\tcurrent-mount-status={}\tsidebar={}\toperation-policy={}\tindex-admission={}\trescan-index={}\treason={}",
            self.kind.as_str(),
            self.native_status.as_str(),
            self.path
                .as_ref()
                .map(|path| escape_field(&path.to_string_lossy()))
                .unwrap_or_else(|| "-".to_string()),
            self.previous_kind.map(VolumeKind::as_str).unwrap_or("-"),
            self.previous_mount_state.map(MountState::as_str).unwrap_or("-"),
            self.previous_case_sensitive
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.previous_native_status
                .map(NativeVolumeStatus::as_str)
                .unwrap_or("-"),
            self.previous_resource_status
                .map(NativeVolumeStatus::as_str)
                .unwrap_or("-"),
            self.previous_mount_table_status
                .map(NativeVolumeStatus::as_str)
                .unwrap_or("-"),
            self.current_kind.map(VolumeKind::as_str).unwrap_or("-"),
            self.current_mount_state.map(MountState::as_str).unwrap_or("-"),
            self.current_case_sensitive
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.current_native_status
                .map(NativeVolumeStatus::as_str)
                .unwrap_or("-"),
            self.current_resource_status
                .map(NativeVolumeStatus::as_str)
                .unwrap_or("-"),
            self.current_mount_table_status
                .map(NativeVolumeStatus::as_str)
                .unwrap_or("-"),
            self.invalidate_sidebar,
            self.invalidate_operation_policy,
            self.invalidate_index_admission,
            self.rescan_index,
            escape_field(&self.reason)
        )
    }
}

fn description_change_invalidates_policy(reason: &str) -> bool {
    !matches!(
        reason,
        "volume-label-changed" | "volume-event-description-changed"
    )
}

impl VolumeEventState {
    pub fn new(mut report: VolumeDiscoveryReport) -> Self {
        normalize_discovered_volumes(&mut report.volumes);
        let mut state = Self {
            report,
            stable_index: BTreeMap::new(),
            path_index: BTreeMap::new(),
        };
        state.rebuild_indexes();
        state
    }

    pub fn discover() -> Self {
        Self::new(VolumeDiscoveryReport::discover())
    }

    pub fn report(&self) -> &VolumeDiscoveryReport {
        &self.report
    }

    pub fn apply_event(&mut self, event: &VolumeEventReport) -> VolumeEventInvalidationReport {
        self.apply_event_transition(event).invalidation
    }

    pub fn apply_event_transition(
        &mut self,
        event: &VolumeEventReport,
    ) -> VolumeEventStateTransition {
        self.apply_parts_transition(
            event.kind,
            event.native_status,
            event.path.clone(),
            event.descriptor.clone(),
            event.reason.clone(),
        )
    }

    pub fn apply_parts(
        &mut self,
        kind: VolumeEventKind,
        native_status: NativeVolumeStatus,
        path: Option<PathBuf>,
        current: Option<VolumeDescriptor>,
        native_reason: Option<String>,
    ) -> VolumeEventInvalidationReport {
        self.apply_parts_transition(kind, native_status, path, current, native_reason)
            .invalidation
    }

    pub fn apply_parts_transition(
        &mut self,
        kind: VolumeEventKind,
        native_status: NativeVolumeStatus,
        path: Option<PathBuf>,
        current: Option<VolumeDescriptor>,
        native_reason: Option<String>,
    ) -> VolumeEventStateTransition {
        let previous = self
            .matching_previous(path.as_deref(), current.as_ref())
            .cloned();
        let invalidation = VolumeEventInvalidationReport::from_transition(
            kind,
            native_status,
            previous.as_ref(),
            current.as_ref(),
            native_reason,
        );
        self.apply_state_change(kind, path.as_deref(), previous.as_ref(), current.clone());
        VolumeEventStateTransition {
            previous,
            current,
            invalidation,
        }
    }

    fn matching_previous(
        &self,
        path: Option<&Path>,
        current: Option<&VolumeDescriptor>,
    ) -> Option<&VolumeDescriptor> {
        current
            .and_then(|current| self.volume_by_stable_identity(&current.stable_identity))
            .or_else(|| path.and_then(|path| self.volume_by_path(path)))
    }

    fn apply_state_change(
        &mut self,
        kind: VolumeEventKind,
        path: Option<&Path>,
        previous: Option<&VolumeDescriptor>,
        current: Option<VolumeDescriptor>,
    ) {
        match kind {
            VolumeEventKind::Appeared | VolumeEventKind::DescriptionChanged => {
                if let Some(current) = current {
                    self.upsert(current);
                }
            }
            VolumeEventKind::Disappeared => {
                self.remove(previous, path);
            }
            VolumeEventKind::Unavailable => {}
        }
    }

    fn upsert(&mut self, current: VolumeDescriptor) {
        if let Some(index) = self
            .stable_index
            .get(current.stable_identity.as_str())
            .copied()
        {
            let previous_path = self.report.volumes[index].path.clone();
            if previous_path != current.path {
                self.path_index.remove(&previous_path);
                self.path_index.insert(current.path.clone(), index);
            }
            self.report.volumes[index] = current;
        } else if let Some(index) = self.path_index.get(&current.path).copied() {
            let previous_identity = self.report.volumes[index].stable_identity.clone();
            self.stable_index.remove(previous_identity.as_str());
            self.stable_index
                .insert(current.stable_identity.clone(), index);
            self.report.volumes[index] = current;
        } else {
            let index = self.report.volumes.len();
            self.stable_index
                .insert(current.stable_identity.clone(), index);
            self.path_index.insert(current.path.clone(), index);
            self.report.volumes.push(current);
        }
    }

    fn remove(&mut self, previous: Option<&VolumeDescriptor>, path: Option<&Path>) {
        if let Some(previous) = previous {
            self.remove_by_stable_identity(&previous.stable_identity);
        } else if let Some(path) = path {
            self.remove_by_path(path);
        }
    }

    fn volume_by_stable_identity(&self, stable_identity: &str) -> Option<&VolumeDescriptor> {
        self.stable_index
            .get(stable_identity)
            .and_then(|index| self.report.volumes.get(*index))
    }

    fn volume_by_path(&self, path: &Path) -> Option<&VolumeDescriptor> {
        self.path_index
            .get(path)
            .and_then(|index| self.report.volumes.get(*index))
    }

    fn remove_by_stable_identity(&mut self, stable_identity: &str) {
        if let Some(index) = self.stable_index.get(stable_identity).copied() {
            self.report.volumes.remove(index);
            self.rebuild_indexes();
        }
    }

    fn remove_by_path(&mut self, path: &Path) {
        if let Some(index) = self.path_index.get(path).copied() {
            self.report.volumes.remove(index);
            self.rebuild_indexes();
        }
    }

    fn rebuild_indexes(&mut self) {
        self.stable_index.clear();
        self.path_index.clear();
        for (index, volume) in self.report.volumes.iter().enumerate() {
            self.stable_index
                .insert(volume.stable_identity.clone(), index);
            self.path_index.insert(volume.path.clone(), index);
        }
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

    pub fn shutdown(self) -> VolumeEventShutdownReport {
        let shutdown = self.stream.shutdown();
        VolumeEventShutdownReport {
            attached_before_shutdown: shutdown.attached_before_shutdown,
            stop_requested: shutdown.stop_requested,
            thread_joined: shutdown.thread_joined,
        }
    }
}

impl VolumeEventShutdownReport {
    pub fn as_tsv(&self) -> String {
        format!(
            "volume-events-shutdown\tattached-before={}\tstop-requested={}\tthread-joined={}",
            self.attached_before_shutdown, self.stop_requested, self.thread_joined
        )
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
    Missing,
    Unsupported,
    Cancelled,
    Unavailable,
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
            Self::Missing => "missing",
            Self::Unsupported => "unsupported",
            Self::Cancelled => "cancelled",
            Self::Unavailable => "unavailable",
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
        match path.try_exists() {
            Ok(true) => {}
            Ok(false) => {
                return Ok(Self::without_volume(
                    path,
                    operation,
                    VolumeOperationDisposition::Missing,
                    "volume-path-missing",
                ));
            }
            Err(err) => {
                return Ok(Self::without_volume(
                    path,
                    operation,
                    VolumeOperationDisposition::Unavailable,
                    format!("volume-path-existence-unavailable: {err}"),
                ));
            }
        }

        let volume = operation_volume_for_path(&path)?;
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
        if !operation_targets_volume_root(&path, &volume.path) {
            return Ok(Self::with_volume_path(
                path,
                operation,
                VolumeOperationDisposition::Refused,
                None,
                None,
                volume,
                "native-volume-operation-requires-volume-root",
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

    fn with_volume_path(
        path: PathBuf,
        operation: VolumeOperation,
        disposition: VolumeOperationDisposition,
        native_status: Option<NativeVolumeOperationStatus>,
        dissenter_status: Option<u32>,
        volume: VolumeDescriptor,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            path,
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

fn operation_volume_for_path(path: &Path) -> Result<VolumeDescriptor> {
    VolumeDiscoveryReport::for_containing_path(path)
        .volume_for_path(path)
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| VolumeDescriptor::for_path(path))
}

fn operation_targets_volume_root(path: &Path, volume_path: &Path) -> bool {
    if path == volume_path {
        return true;
    }
    match (
        normalized_lookup_path(path),
        normalized_lookup_path(volume_path),
    ) {
        (Some(path), Some(volume_path)) => path == volume_path,
        _ => false,
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
        NativeVolumeOperationStatus::Error => VolumeOperationDisposition::Failed,
        NativeVolumeOperationStatus::Busy
        | NativeVolumeOperationStatus::ExclusiveAccess
        | NativeVolumeOperationStatus::NoResources
        | NativeVolumeOperationStatus::NotReady => VolumeOperationDisposition::Busy,
        NativeVolumeOperationStatus::NotPermitted | NativeVolumeOperationStatus::NotPrivileged => {
            VolumeOperationDisposition::Denied
        }
        NativeVolumeOperationStatus::Unsupported => VolumeOperationDisposition::Unsupported,
        NativeVolumeOperationStatus::BadArgument
        | NativeVolumeOperationStatus::NotMounted
        | NativeVolumeOperationStatus::NotWritable => VolumeOperationDisposition::Refused,
        NativeVolumeOperationStatus::NotFound | NativeVolumeOperationStatus::Missing => {
            VolumeOperationDisposition::Missing
        }
        NativeVolumeOperationStatus::Cancelled => VolumeOperationDisposition::Cancelled,
        NativeVolumeOperationStatus::Failed => VolumeOperationDisposition::Failed,
        NativeVolumeOperationStatus::Unavailable => VolumeOperationDisposition::Unavailable,
    }
}

fn disabled_command_reason(
    operation: VolumeOperation,
    volume: &VolumeDescriptor,
) -> Option<String> {
    let state = match operation {
        VolumeOperation::Eject => volume.commands.eject,
        VolumeOperation::Unmount => volume.commands.unmount,
        VolumeOperation::Mount => volume.commands.mount,
    };
    (state != VolumeCommandState::Enabled).then(|| match operation {
        VolumeOperation::Eject | VolumeOperation::Unmount => volume
            .commands
            .reason
            .clone()
            .unwrap_or_else(|| format!("{}-command-disabled", operation.as_str())),
        VolumeOperation::Mount => "mount-command-disabled".to_string(),
    })
}

fn normalized_volume_map(volumes: &[VolumeDescriptor]) -> BTreeMap<String, &VolumeDescriptor> {
    let mut volumes: Vec<_> = volumes.iter().collect();
    volumes.sort_by(|left, right| compare_volume_descriptors(left, right));
    let mut seen = BTreeSet::new();
    volumes
        .into_iter()
        .filter(|volume| seen.insert(volume.stable_identity.clone()))
        .map(|volume| (volume.stable_identity.clone(), volume))
        .collect()
}

fn unique_volume_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

fn normalize_discovered_volumes(volumes: &mut Vec<VolumeDescriptor>) {
    volumes.sort_by(compare_volume_descriptors);
    let mut seen_identity = BTreeSet::new();
    let mut seen_descriptor = BTreeSet::new();
    volumes.retain(|volume| {
        seen_descriptor.insert((volume.id, volume.path.clone()))
            && seen_identity.insert(volume.stable_identity.clone())
    });
}

fn compare_volume_descriptors(
    left: &VolumeDescriptor,
    right: &VolumeDescriptor,
) -> std::cmp::Ordering {
    left.path
        .cmp(&right.path)
        .then(left.label.cmp(&right.label))
        .then(left.id.cmp(&right.id))
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
    } else if previous.removable != current.removable {
        Some("volume-media-truth-changed")
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
    } else if previous.case_sensitive != current.case_sensitive {
        Some("volume-case-sensitivity-changed")
    } else if previous.native_status != current.native_status
        || previous.resource_status != current.resource_status
        || previous.mount_table_status != current.mount_table_status
    {
        Some("volume-api-status-changed")
    } else if previous.apfs_container_uuid != current.apfs_container_uuid
        || previous.apfs_role != current.apfs_role
    {
        Some("volume-apfs-metadata-changed")
    } else if previous.mount_filesystem != current.mount_filesystem
        || previous.mount_flags != current.mount_flags
        || previous.mount_local != current.mount_local
        || previous.mount_read_only != current.mount_read_only
    {
        Some("volume-mount-table-changed")
    } else if previous.filesystem != current.filesystem
        || previous.volume_type != current.volume_type
        || previous.media_kind != current.media_kind
        || previous.media_type != current.media_type
        || previous.media_leaf != current.media_leaf
        || previous.media_whole != current.media_whole
        || previous.media_encrypted != current.media_encrypted
        || previous.media_block_size_bytes != current.media_block_size_bytes
        || previous.media_size_bytes != current.media_size_bytes
        || previous.case_preserving != current.case_preserving
        || previous.resource_automounted != current.resource_automounted
        || previous.resource_browsable != current.resource_browsable
        || previous.resource_encrypted != current.resource_encrypted
        || previous.resource_reachable != current.resource_reachable
        || previous.resource_root_file_system != current.resource_root_file_system
        || previous.resource_supports_file_cloning != current.resource_supports_file_cloning
        || previous.resource_supports_hard_links != current.resource_supports_hard_links
        || previous.resource_supports_sparse_files != current.resource_supports_sparse_files
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

fn apfs_container_uuid(
    native: Option<&NativeVolumeDescription>,
    _mount_table: Option<&NativeVolumeMountTableEntry>,
) -> Option<String> {
    let native = native.filter(|native| native.status == NativeVolumeStatus::Available)?;
    if !native_has_apfs_evidence(native) {
        return None;
    }
    native
        .whole_disk_media_uuid
        .clone()
        .filter(|uuid| native.volume_uuid.as_ref() != Some(uuid))
}

fn apfs_volume_role(native: Option<&NativeVolumeDescription>) -> Option<ApfsVolumeRole> {
    let native = native.filter(|native| native.status == NativeVolumeStatus::Available)?;
    [
        native.volume_type.as_deref(),
        native.volume_kind.as_deref(),
        native.media_content.as_deref(),
        native.media_type.as_deref(),
    ]
    .into_iter()
    .flatten()
    .find_map(parse_apfs_role)
}

fn native_has_apfs_evidence(native: &NativeVolumeDescription) -> bool {
    [
        native.volume_type.as_deref(),
        native.volume_kind.as_deref(),
        native.media_content.as_deref(),
        native.media_type.as_deref(),
        native.media_kind.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| value.to_ascii_lowercase().contains("apfs"))
}

fn parse_apfs_role(value: &str) -> Option<ApfsVolumeRole> {
    let normalized = value
        .trim()
        .trim_matches(|ch: char| ch == '"' || ch == '\'' || ch == '(' || ch == ')')
        .to_ascii_lowercase()
        .replace(['_', '-'], " ");
    let mut tokens = normalized.split_whitespace().collect::<Vec<_>>();
    if tokens.is_empty() {
        return None;
    }
    if tokens.first() == Some(&"apple") && tokens.get(1) == Some(&"apfs") {
        tokens.drain(..2);
    } else if tokens.first() == Some(&"apfs") {
        tokens.drain(..1);
    }
    let role = tokens
        .iter()
        .find(|token| **token != "volume" && **token != "role")
        .copied()?;
    match role {
        "system" => Some(ApfsVolumeRole::System),
        "data" => Some(ApfsVolumeRole::Data),
        "preboot" => Some(ApfsVolumeRole::Preboot),
        "recovery" => Some(ApfsVolumeRole::Recovery),
        "vm" => Some(ApfsVolumeRole::Vm),
        "update" => Some(ApfsVolumeRole::Update),
        "xart" => Some(ApfsVolumeRole::Xart),
        "hardware" => Some(ApfsVolumeRole::Hardware),
        "backup" => Some(ApfsVolumeRole::Backup),
        "unknown" => Some(ApfsVolumeRole::Unknown),
        _ => None,
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
        .or_else(|| path.try_exists().ok())
}

fn volume_network_state(
    marker: Option<&str>,
    native: Option<&NativeVolumeDescription>,
    resource: Option<&NativeVolumeResourceValues>,
    mount_table: Option<&NativeVolumeMountTableEntry>,
    kind: VolumeKind,
) -> bool {
    if let Some(network) = marker_network(marker) {
        return network;
    }
    if native
        .filter(|native| native.status == NativeVolumeStatus::Available)
        .and_then(|native| native.volume_network)
        == Some(true)
    {
        return true;
    }
    mount_table
        .filter(|mount_table| mount_table.status == NativeVolumeStatus::Available)
        .and_then(|mount_table| mount_table.is_local)
        .or_else(|| {
            resource
                .filter(|resource| resource.status == NativeVolumeStatus::Available)
                .and_then(|resource| resource.is_local)
        })
        .map(|local| !local)
        .or_else(|| {
            native
                .filter(|native| native.status == NativeVolumeStatus::Available)
                .and_then(|native| native.volume_network)
        })
        .unwrap_or(kind == VolumeKind::Network)
}

fn volume_local_state(
    marker: Option<&str>,
    native: Option<&NativeVolumeDescription>,
    resource: Option<&NativeVolumeResourceValues>,
    mount_table: Option<&NativeVolumeMountTableEntry>,
    kind: VolumeKind,
) -> Option<bool> {
    if let Some(network) = marker_network(marker) {
        return Some(!network);
    }
    if native
        .filter(|native| native.status == NativeVolumeStatus::Available)
        .and_then(|native| native.volume_network)
        == Some(true)
    {
        return Some(false);
    }
    mount_table
        .filter(|mount_table| mount_table.status == NativeVolumeStatus::Available)
        .and_then(|mount_table| mount_table.is_local)
        .or_else(|| {
            resource
                .filter(|resource| resource.status == NativeVolumeStatus::Available)
                .and_then(|resource| resource.is_local)
        })
        .or_else(|| {
            native
                .filter(|native| native.status == NativeVolumeStatus::Available)
                .and_then(|native| native.volume_network)
                .map(|network| !network)
        })
        .or_else(|| (kind == VolumeKind::Network).then_some(false))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VolumeAccessState {
    writable: bool,
    read_only: bool,
}

fn volume_access_state(
    marker: Option<&str>,
    native: Option<&NativeVolumeDescription>,
    resource: Option<&NativeVolumeResourceValues>,
    mount_table: Option<&NativeVolumeMountTableEntry>,
    filesystem_writable: bool,
) -> VolumeAccessState {
    if let Some(read_only) = marker_read_only(marker) {
        return VolumeAccessState {
            writable: !read_only,
            read_only,
        };
    }
    if let Some(read_only) = mount_table
        .filter(|mount_table| mount_table.status == NativeVolumeStatus::Available)
        .and_then(|mount_table| mount_table.is_read_only)
    {
        return VolumeAccessState {
            writable: !read_only,
            read_only,
        };
    }
    if let Some(read_only) = resource
        .filter(|resource| resource.status == NativeVolumeStatus::Available)
        .and_then(|resource| resource.is_read_only)
    {
        return VolumeAccessState {
            writable: !read_only,
            read_only,
        };
    }
    if let Some(writable) = native
        .filter(|native| native.status == NativeVolumeStatus::Available)
        .and_then(|native| native.media_writable)
    {
        return VolumeAccessState {
            writable,
            read_only: !writable,
        };
    }
    VolumeAccessState {
        writable: filesystem_writable,
        read_only: !filesystem_writable,
    }
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
        Some("external") | Some("external-removable") | Some("external-removable-read-only") => {
            return VolumeKind::External;
        }
        Some("removable") => return VolumeKind::Removable,
        Some("disk-image") => return VolumeKind::DiskImage,
        Some("system") => return VolumeKind::System,
        Some("internal") => return VolumeKind::Internal,
        _ => {}
    }

    if let Some(kind) = classify_native_volume(path, native, resource, mount_table) {
        return kind;
    }
    if native_volume_evidence_unavailable(native, resource, mount_table) {
        return VolumeKind::Unknown;
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
    let native = native.filter(|native| native.status == NativeVolumeStatus::Available);
    let resource = resource.filter(|resource| resource.status == NativeVolumeStatus::Available);
    if resource.and_then(|resource| resource.is_root_file_system) == Some(true) {
        return Some(VolumeKind::System);
    }
    if path == Path::new("/") {
        return Some(VolumeKind::System);
    }
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

    if resource.and_then(|resource| resource.is_local) == Some(false) {
        return Some(VolumeKind::Network);
    }
    let mount_table =
        mount_table.filter(|mount_table| mount_table.status == NativeVolumeStatus::Available);
    if mount_table.and_then(|mount_table| mount_table.is_local) == Some(false) {
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

fn native_volume_evidence_unavailable(
    native: Option<&NativeVolumeDescription>,
    resource: Option<&NativeVolumeResourceValues>,
    mount_table: Option<&NativeVolumeMountTableEntry>,
) -> bool {
    let statuses = [
        native.map(|native| native.status),
        resource.map(|resource| resource.status),
        mount_table.map(|mount_table| mount_table.status),
    ];
    statuses.iter().any(Option::is_some)
        && statuses
            .iter()
            .flatten()
            .all(|status| *status != NativeVolumeStatus::Available)
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
    if paths.is_empty() && path.try_exists().ok() == Some(false) {
        paths = path
            .ancestors()
            .find(|ancestor| ancestor.try_exists().ok() == Some(true))
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
            if fallback_volume_path_is_directory(&path) == Some(true) {
                paths.push(path);
            }
        }
    }
    paths
}

fn fallback_volume_path_is_directory(path: &Path) -> Option<bool> {
    match fs::metadata(path) {
        Ok(metadata) => Some(metadata.is_dir()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Some(false),
        Err(_) => None,
    }
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
    if !marker_fixture_path_allowed(path) {
        return None;
    }
    let value = fs::read_to_string(path.join(VOLUME_MARKER)).ok()?;
    let value = value.lines().next()?.trim().to_ascii_lowercase();
    (!value.is_empty()).then_some(value)
}

fn marker_fixture_path_allowed(path: &Path) -> bool {
    let Ok(temp_dir) = std::env::temp_dir().canonicalize() else {
        return false;
    };
    let Ok(path) = path.canonicalize() else {
        return false;
    };
    path.starts_with(&temp_dir)
        && path
            .components()
            .any(|component| component.as_os_str().to_string_lossy().starts_with("gfm-"))
}

fn marker_removable(marker: Option<&str>) -> Option<bool> {
    match marker {
        Some("external-removable")
        | Some("external-removable-read-only")
        | Some("removable")
        | Some("disk-image") => Some(true),
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
        | Some("external-removable-read-only")
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
        | Some("external-removable-read-only")
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

fn marker_read_only(marker: Option<&str>) -> Option<bool> {
    match marker {
        Some("external-removable-read-only") => Some(true),
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

fn volume_match_depth(
    volume: &VolumeDescriptor,
    path: &Path,
    normalized_path: Option<&Path>,
) -> Option<usize> {
    let direct_match = path.starts_with(&volume.path);
    let normalized_volume_path = (direct_match || normalized_path.is_some())
        .then(|| normalized_lookup_path(&volume.path))
        .flatten();
    if direct_match {
        return Some(
            normalized_volume_path
                .as_deref()
                .unwrap_or(&volume.path)
                .components()
                .count(),
        );
    }
    let normalized_path = normalized_path?;
    let normalized_volume_path = normalized_volume_path?;
    normalized_path
        .starts_with(&normalized_volume_path)
        .then(|| normalized_volume_path.components().count())
}

fn normalized_lookup_path(path: &Path) -> Option<PathBuf> {
    if let Ok(canonical) = path.canonicalize() {
        return Some(canonical);
    }

    let mut candidate = path;
    let mut missing = Vec::new();
    loop {
        match candidate.try_exists() {
            Ok(true) => {
                let mut normalized = candidate.canonicalize().ok()?;
                for component in missing.iter().rev() {
                    normalized.push(component);
                }
                return Some(normalized);
            }
            Ok(false) => {}
            Err(_) => return None,
        }
        missing.push(candidate.file_name()?.to_os_string());
        candidate = candidate.parent()?;
    }
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

    #[cfg(unix)]
    use std::ffi::OsString;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;

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
        assert!(descriptor.as_tsv().contains("\tresource-encrypted="));
        assert!(descriptor.as_tsv().contains("\tresource-reachable=true\t"));
        assert!(descriptor
            .as_tsv()
            .contains("\tresource-root-filesystem=true\t"));
        assert!(descriptor
            .as_tsv()
            .contains("\tresource-supports-file-cloning="));
        assert!(descriptor
            .as_tsv()
            .contains("\tresource-supports-hard-links="));
        assert!(descriptor
            .as_tsv()
            .contains("\tresource-supports-sparse-files="));
        assert!(descriptor.as_tsv().contains("\tresource-remount-url="));
        assert!(descriptor.as_tsv().contains("\tmount-status=available\t"));
        assert!(descriptor.as_tsv().contains("\tapfs-container-uuid="));
        assert!(descriptor.as_tsv().contains("\tapfs-role="));
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
    fn native_resource_root_filesystem_classifies_system_without_path_fallback() {
        let path = Path::new("/Volumes/SystemSnapshot");
        let resource = resource_values(|values| {
            values.is_root_file_system = Some(true);
            values.is_internal = Some(true);
        });

        let kind = classify_volume(path, None, None, Some(&resource), None);

        assert_eq!(kind, VolumeKind::System);
    }

    #[test]
    fn ignores_volume_markers_outside_fixture_roots() {
        let root = std::env::temp_dir().join(format!(
            "ordinary-volume-marker-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::SeqCst),
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(VOLUME_MARKER), "network-unreachable\n").unwrap();

        let descriptor = VolumeDescriptor::for_path(&root).unwrap();

        assert_ne!(descriptor.kind, VolumeKind::Network);
        assert_eq!(descriptor.reachable, Some(true));
        assert!(!descriptor.source.contains("fixture-marker"));
        assert!(!descriptor.stable_identity.starts_with("fixture-marker:"));

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
    fn classify_native_volume_prefers_mount_table_network_over_resource_ejectability() {
        let resource = resource_values(|values| {
            values.is_ejectable = Some(true);
        });
        let mount_table = mount_table_entry(|entry| {
            entry.is_local = Some(false);
        });

        let kind = classify_native_volume(
            Path::new("/Volumes/Team Share"),
            None,
            Some(&resource),
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
    fn classify_volume_reports_unknown_when_all_native_evidence_is_unavailable() {
        let native = native_description(|description| {
            description.status = NativeVolumeStatus::Unavailable;
            description.reason = Some("DiskArbitration unavailable".to_string());
        });
        let resource = resource_values(|values| {
            values.status = NativeVolumeStatus::Unavailable;
            values.reason = Some("URL resource unavailable".to_string());
        });
        let mount_table = mount_table_entry(|entry| {
            entry.status = NativeVolumeStatus::Unavailable;
            entry.reason = Some("mount table unavailable".to_string());
        });

        let kind = classify_volume(
            Path::new("/Volumes/Team SMB"),
            None,
            Some(&native),
            Some(&resource),
            Some(&mount_table),
        );

        assert_eq!(kind, VolumeKind::Unknown);
    }

    #[test]
    fn descriptor_reports_platform_state_unavailable_only_when_all_volume_apis_fail() {
        let root = unique_temp_dir("gfm-volume-platform-state-unavailable");
        let mut descriptor = VolumeDescriptor::for_path(&root).unwrap();

        descriptor.native_status = Some(NativeVolumeStatus::Unavailable);
        descriptor.resource_status = Some(NativeVolumeStatus::Unavailable);
        descriptor.mount_table_status = Some(NativeVolumeStatus::Unavailable);
        assert!(descriptor.platform_state_unavailable());

        descriptor.mount_table_status = Some(NativeVolumeStatus::Available);
        assert!(!descriptor.platform_state_unavailable());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn classify_volume_keeps_path_fallback_only_when_native_evidence_is_absent() {
        let kind = classify_volume(Path::new("/Volumes/Team SMB"), None, None, None, None);

        assert_eq!(kind, VolumeKind::Network);
    }

    #[test]
    fn volume_network_state_prefers_positive_diskarbitration_network_truth() {
        let native = native_description(|description| {
            description.volume_network = Some(true);
        });
        let resource = resource_values(|values| {
            values.is_local = Some(true);
        });
        let mount_table = mount_table_entry(|entry| {
            entry.is_local = Some(true);
        });

        assert!(volume_network_state(
            None,
            Some(&native),
            Some(&resource),
            Some(&mount_table),
            VolumeKind::Network,
        ));
    }

    #[test]
    fn volume_local_state_prefers_positive_diskarbitration_network_truth() {
        let native = native_description(|description| {
            description.volume_network = Some(true);
        });
        let resource = resource_values(|values| {
            values.is_local = Some(true);
        });
        let mount_table = mount_table_entry(|entry| {
            entry.is_local = Some(true);
        });

        assert_eq!(
            volume_local_state(
                None,
                Some(&native),
                Some(&resource),
                Some(&mount_table),
                VolumeKind::Network,
            ),
            Some(false)
        );
    }

    #[test]
    fn volume_network_state_uses_mount_table_network_when_diskarbitration_is_local() {
        let native = native_description(|description| {
            description.volume_network = Some(false);
        });
        let mount_table = mount_table_entry(|entry| {
            entry.is_local = Some(false);
        });

        assert!(volume_network_state(
            None,
            Some(&native),
            None,
            Some(&mount_table),
            VolumeKind::Internal,
        ));
    }

    #[test]
    fn volume_local_state_uses_mount_table_network_when_diskarbitration_is_local() {
        let native = native_description(|description| {
            description.volume_network = Some(false);
        });
        let mount_table = mount_table_entry(|entry| {
            entry.is_local = Some(false);
        });

        assert_eq!(
            volume_local_state(
                None,
                Some(&native),
                None,
                Some(&mount_table),
                VolumeKind::Internal,
            ),
            Some(false)
        );
    }

    #[test]
    fn promotes_native_apfs_whole_disk_uuid_to_container_identity() {
        let native = native_description(|description| {
            description.status = NativeVolumeStatus::Available;
            description.volume_type = Some("apfs".to_string());
            description.media_content = Some("Apple_APFS_Role_Data".to_string());
            description.volume_uuid = Some("APFS-VOLUME-UUID".to_string());
            description.media_uuid = Some("APFS-MEDIA-UUID".to_string());
            description.whole_disk_media_uuid = Some("APFS-CONTAINER-UUID".to_string());
        });
        let mount_table = mount_table_entry(|entry| {
            entry.filesystem_type = Some("apfs".to_string());
        });

        assert_eq!(
            apfs_container_uuid(Some(&native), Some(&mount_table)).as_deref(),
            Some("APFS-CONTAINER-UUID")
        );
        assert_eq!(apfs_volume_role(Some(&native)), Some(ApfsVolumeRole::Data));
    }

    #[test]
    fn leaves_apfs_container_uuid_unknown_without_native_container_source() {
        let native = native_description(|description| {
            description.status = NativeVolumeStatus::Available;
            description.volume_type = Some("apfs".to_string());
            description.media_content = Some("Apple_APFS_Role_Data".to_string());
            description.volume_uuid = Some("APFS-VOLUME-UUID".to_string());
            description.media_uuid = Some("APFS-MEDIA-UUID".to_string());
        });

        assert_eq!(apfs_container_uuid(Some(&native), None), None);
    }

    #[test]
    fn leaves_apfs_metadata_unknown_without_native_apfs_evidence() {
        let native = native_description(|description| {
            description.status = NativeVolumeStatus::Unavailable;
            description.media_content = Some("Apple_APFS_Role_System".to_string());
            description.media_uuid = Some("APFS-CONTAINER-UUID".to_string());
            description.whole_disk_media_uuid = Some("APFS-CONTAINER-UUID".to_string());
            description.reason = Some("DiskArbitration unavailable".to_string());
        });

        assert_eq!(apfs_container_uuid(Some(&native), None), None);
        assert_eq!(apfs_volume_role(Some(&native)), None);
        assert_eq!(parse_apfs_role("plain external disk"), None);
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
    fn volume_discovery_deduplicates_input_paths_before_descriptor_output() {
        let root = unique_temp_dir("gfm-volume-discovery-dedup");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();

        let report =
            VolumeDiscoveryReport::from_paths(vec![root.clone(), root.clone(), root.clone()]);

        assert_eq!(report.volumes.len(), 1);
        assert_eq!(report.volumes[0].path, root);
        assert_eq!(report.volumes[0].kind, VolumeKind::External);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn checked_volume_discovery_deduplicates_input_paths_before_descriptor_output() {
        let root = unique_temp_dir("gfm-volume-discovery-checked-dedup");
        fs::write(root.join(VOLUME_MARKER), "network-smb\n").unwrap();

        let report = VolumeDiscoveryReport::from_paths_checked(vec![
            root.clone(),
            root.clone(),
            root.clone(),
        ])
        .unwrap();

        assert_eq!(report.volumes.len(), 1);
        assert_eq!(report.volumes[0].path, root);
        assert_eq!(report.volumes[0].kind, VolumeKind::Network);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn volume_discovery_deduplicates_stable_identities_before_topology_maps() {
        let first = unique_temp_dir("gfm-volume-discovery-stable-first");
        let second = unique_temp_dir("gfm-volume-discovery-stable-second");
        fs::write(first.join(VOLUME_MARKER), "external-removable\n").unwrap();
        fs::write(second.join(VOLUME_MARKER), "network-smb\n").unwrap();
        let mut first_volume = VolumeDescriptor::for_path(&first).unwrap();
        let mut second_volume = VolumeDescriptor::for_path(&second).unwrap();
        first_volume.stable_identity = "diskarbitration:uuid:DUPLICATE".to_string();
        second_volume.stable_identity = first_volume.stable_identity.clone();
        let expected_path = first_volume.path.clone().min(second_volume.path.clone());
        let mut volumes = vec![second_volume, first_volume];

        normalize_discovered_volumes(&mut volumes);

        assert_eq!(volumes.len(), 1);
        assert_eq!(volumes[0].stable_identity, "diskarbitration:uuid:DUPLICATE");
        assert_eq!(volumes[0].path, expected_path);

        fs::remove_dir_all(first).unwrap();
        fs::remove_dir_all(second).unwrap();
    }

    #[test]
    fn topology_diff_normalizes_duplicate_stable_identities_before_mapping() {
        let first = unique_temp_dir("gfm-volume-topology-duplicate-first");
        let second = unique_temp_dir("gfm-volume-topology-duplicate-second");
        fs::write(first.join(VOLUME_MARKER), "external-removable\n").unwrap();
        fs::write(second.join(VOLUME_MARKER), "network-smb\n").unwrap();
        let mut first_volume = VolumeDescriptor::for_path(&first).unwrap();
        let mut second_volume = VolumeDescriptor::for_path(&second).unwrap();
        first_volume.stable_identity = "diskarbitration:uuid:DUPLICATE-TOPOLOGY".to_string();
        second_volume.stable_identity = first_volume.stable_identity.clone();
        let retained = if first_volume.path <= second_volume.path {
            first_volume.clone()
        } else {
            second_volume.clone()
        };
        let previous = VolumeDiscoveryReport {
            volumes: vec![second_volume, first_volume],
        };
        let mut current_volume = retained.clone();
        current_volume.label = "Renamed Duplicate".to_string();
        let current = VolumeDiscoveryReport {
            volumes: vec![current_volume],
        };

        let diff = VolumeTopologyDiff::evaluate(&previous, &current);

        assert_eq!(diff.changes.len(), 1);
        assert_eq!(
            diff.changes[0].stable_identity,
            "diskarbitration:uuid:DUPLICATE-TOPOLOGY"
        );
        assert_eq!(diff.changes[0].path, retained.path);
        assert_eq!(diff.changes[0].reason, "volume-label-changed");

        fs::remove_dir_all(first).unwrap();
        fs::remove_dir_all(second).unwrap();
    }

    #[test]
    fn volume_event_state_normalizes_duplicate_stable_identities_before_indexing() {
        let first = unique_temp_dir("gfm-volume-state-duplicate-first");
        let second = unique_temp_dir("gfm-volume-state-duplicate-second");
        fs::write(first.join(VOLUME_MARKER), "external-removable\n").unwrap();
        fs::write(second.join(VOLUME_MARKER), "network-smb\n").unwrap();
        let mut first_volume = VolumeDescriptor::for_path(&first).unwrap();
        let mut second_volume = VolumeDescriptor::for_path(&second).unwrap();
        first_volume.stable_identity = "diskarbitration:uuid:DUPLICATE-STATE".to_string();
        second_volume.stable_identity = first_volume.stable_identity.clone();
        let retained = if first_volume.path <= second_volume.path {
            first_volume.clone()
        } else {
            second_volume.clone()
        };

        let state = VolumeEventState::new(VolumeDiscoveryReport {
            volumes: vec![second_volume, first_volume],
        });

        assert_eq!(state.report().volumes.len(), 1);
        assert_eq!(
            state.report().volumes[0].stable_identity,
            retained.stable_identity
        );
        assert_eq!(state.report().volumes[0].path, retained.path);

        fs::remove_dir_all(first).unwrap();
        fs::remove_dir_all(second).unwrap();
    }

    #[test]
    fn volume_lookup_prefers_deepest_matching_volume() {
        let root = unique_temp_dir("gfm-volume-deepest-root");
        let nested_root = root.join("Nested Volume");
        let file = nested_root.join("Project").join("Plan.md");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();
        fs::write(nested_root.join(VOLUME_MARKER), "network-smb\n").unwrap();
        fs::write(&file, "plan").unwrap();
        let root_volume = VolumeDescriptor::for_path(&root).unwrap();
        let nested_volume = VolumeDescriptor::for_path(&nested_root).unwrap();
        let report = VolumeDiscoveryReport {
            volumes: vec![root_volume, nested_volume.clone()],
        };

        let volume = report
            .volume_for_path(&file)
            .expect("nested file should resolve to a containing volume");

        assert_eq!(volume.path, nested_root);
        assert_eq!(volume.kind, VolumeKind::Network);
        assert_eq!(volume.stable_identity, nested_volume.stable_identity);

        fs::remove_dir_all(root).unwrap();
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
    fn volume_access_prefers_fixture_read_only_marker_over_host_mount_table() {
        let mount_table = mount_table_entry(|entry| {
            entry.is_read_only = Some(false);
        });
        let resource = resource_values(|values| {
            values.is_read_only = Some(true);
        });
        let native = native_description(|description| {
            description.media_writable = Some(false);
        });

        let access = volume_access_state(
            Some("external-removable-read-only"),
            Some(&native),
            Some(&resource),
            Some(&mount_table),
            false,
        );

        assert_eq!(
            access,
            VolumeAccessState {
                writable: false,
                read_only: true
            }
        );
    }

    #[test]
    fn volume_access_uses_marker_only_when_native_access_is_unknown() {
        let access = volume_access_state(
            Some("external-removable-read-only"),
            Some(&native_description(|description| {
                description.status = NativeVolumeStatus::Unavailable;
                description.media_writable = Some(true);
            })),
            Some(&resource_values(|values| {
                values.status = NativeVolumeStatus::Unavailable;
                values.is_read_only = Some(false);
            })),
            Some(&mount_table_entry(|entry| {
                entry.status = NativeVolumeStatus::Unavailable;
                entry.is_read_only = Some(false);
            })),
            true,
        );

        assert_eq!(
            access,
            VolumeAccessState {
                writable: false,
                read_only: true
            }
        );
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
    fn volume_operation_refuses_disabled_command_with_policy_reason() {
        let root = unique_temp_dir("gfm-volume-operation-policy");
        fs::write(root.join(VOLUME_MARKER), "internal\n").unwrap();

        let report = VolumeOperationReport::execute(&root, VolumeOperation::Eject).unwrap();

        assert_eq!(report.disposition, VolumeOperationDisposition::Refused);
        assert_eq!(report.native_status, None);
        assert_eq!(report.dissenter_status, None);
        assert_eq!(report.reason, "internal-volume-not-ejectable");
        assert!(report.as_tsv().contains("\tvolume-kind=internal\t"));
        assert!(report
            .as_tsv()
            .contains("\treason=internal-volume-not-ejectable"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn volume_operation_refuses_nested_path_before_native_call() {
        let root = unique_temp_dir("gfm-volume-operation-nested");
        let nested = root.join("Project").join("Preview.pdf");
        fs::create_dir_all(nested.parent().unwrap()).unwrap();
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();
        fs::write(&nested, "%PDF-1.7\n").unwrap();

        let report = VolumeOperationReport::execute(&nested, VolumeOperation::Eject).unwrap();

        assert_eq!(report.path, nested);
        assert_eq!(report.disposition, VolumeOperationDisposition::Refused);
        assert_eq!(report.native_status, None);
        assert_eq!(report.dissenter_status, None);
        assert_eq!(
            report.reason,
            "native-volume-operation-requires-volume-root"
        );
        assert_eq!(
            report.volume.as_ref().map(|volume| volume.path.as_path()),
            Some(root.as_path())
        );
        assert!(report.as_tsv().contains("\tvolume-kind=external\t"));

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
    fn native_missing_volume_operation_status_maps_to_missing_disposition() {
        assert_eq!(
            disposition_for_native_operation(NativeVolumeOperationStatus::Missing),
            VolumeOperationDisposition::Missing
        );
        assert_eq!(
            disposition_for_native_operation(NativeVolumeOperationStatus::NotFound),
            VolumeOperationDisposition::Missing
        );
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
    fn volume_mount_identity_surfaces_native_dissenter_reason() {
        let report = VolumeMountIdentityReport::execute("disk999999s999");

        assert_eq!(report.bsd_name, "disk999999s999");
        assert_eq!(report.disposition, VolumeOperationDisposition::Refused);
        assert_eq!(
            report.native_status,
            NativeVolumeOperationStatus::BadArgument
        );
        assert_eq!(report.dissenter_status, Some(0xf8da0003));
        assert!(
            report
                .reason
                .starts_with("diskarbitration-bad-argument:0xf8da0003"),
            "{}",
            report.reason
        );
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
    fn checked_discovery_preserves_descriptor_probe_failures() {
        let root = unique_temp_dir("gfm-volume-checked-discovery");
        let invalid = root.join("volume-discovery-unavailable".repeat(16));

        let err = VolumeDiscoveryReport::from_paths_checked(vec![invalid]).unwrap_err();

        assert!(err.to_string().contains("File name too long"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fallback_volume_path_directory_probe_preserves_unavailable_state() {
        let root = unique_temp_dir("gfm-volume-fallback-dir-probe");
        let file = root.join("plain.txt");
        let missing = root.join("missing");
        let unprobeable = root.join("volume-fallback-unavailable".repeat(16));
        fs::write(&file, "plain").unwrap();

        assert_eq!(fallback_volume_path_is_directory(&root), Some(true));
        assert_eq!(fallback_volume_path_is_directory(&file), Some(false));
        assert_eq!(fallback_volume_path_is_directory(&missing), Some(false));
        assert_eq!(fallback_volume_path_is_directory(&unprobeable), None);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn normalized_lookup_path_preserves_missing_suffix_under_existing_ancestor() {
        let root = unique_temp_dir("gfm-volume-normalized-missing");
        let missing = root.join("Missing").join("Nested").join("Plan.md");

        let normalized = normalized_lookup_path(&missing).unwrap();

        assert!(normalized.starts_with(root.canonicalize().unwrap()));
        assert!(normalized.ends_with(Path::new("Missing/Nested/Plan.md")));
        assert!(!missing.exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn normalized_lookup_path_returns_unknown_for_unprobeable_component() {
        let root = unique_temp_dir("gfm-volume-normalized-unprobeable");
        let unprobeable = root.join("volume-path-unavailable".repeat(16));

        assert_eq!(normalized_lookup_path(&unprobeable), None);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn network_reachability_returns_unknown_for_unprobeable_fallback_path() {
        let root = unique_temp_dir("gfm-volume-reachability-unprobeable");
        let unprobeable = root.join("volume-reachability-unavailable".repeat(16));

        let reachable = volume_reachability(true, MountState::Mounted, None, &unprobeable);

        assert_eq!(reachable, None);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn volume_lookup_matches_var_private_var_aliases() {
        let root = unique_temp_dir("gfm-volume-private-var-alias");
        let canonical_root = root.canonicalize().unwrap();
        let Some(alias_root) = private_var_alias_for(&canonical_root) else {
            fs::remove_dir_all(root).unwrap();
            return;
        };
        let descriptor = VolumeDescriptor::for_path(&canonical_root).unwrap();
        let report = VolumeDiscoveryReport {
            volumes: vec![descriptor],
        };
        let alias_child = alias_root.join("Nested").join("File.txt");

        let volume = report
            .volume_for_path(&alias_child)
            .expect("canonical volume should contain /var alias child");

        assert_eq!(volume.path, canonical_root);
        assert!(!alias_child.exists());

        fs::remove_dir_all(alias_root).unwrap();
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
        current_volume.apfs_container_uuid = Some("APFS-CONTAINER-UUID".to_string());
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
        let mut previous_volume = VolumeDescriptor::for_path(&root).unwrap();
        previous_volume.apfs_role = Some(ApfsVolumeRole::Data);
        previous_volume.mount_filesystem = Some("apfs".to_string());
        previous_volume.mount_flags = Some(0x0000_1000);
        previous_volume.mount_local = Some(true);
        let mut current_volume = previous_volume.clone();
        current_volume.filesystem = Some("apfs".to_string());
        current_volume.case_preserving = Some(true);
        current_volume.resource_encrypted = Some(true);
        current_volume.resource_reachable = Some(true);
        current_volume.resource_supports_file_cloning = Some(true);
        current_volume.resource_supports_hard_links = Some(true);
        current_volume.resource_supports_sparse_files = Some(true);
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
    fn topology_diff_invalidates_policy_for_root_filesystem_resource_changes() {
        let root = unique_temp_dir("gfm-volume-topology-root-filesystem");
        fs::write(root.join(VOLUME_MARKER), "system\n").unwrap();
        let mut previous_volume = VolumeDescriptor::for_path(&root).unwrap();
        previous_volume.resource_root_file_system = Some(false);
        let mut current_volume = previous_volume.clone();
        current_volume.resource_root_file_system = Some(true);
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
    fn topology_diff_reports_native_apfs_metadata_changes() {
        let root = unique_temp_dir("gfm-volume-topology-apfs-metadata");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();
        let mut previous_volume = VolumeDescriptor::for_path(&root).unwrap();
        previous_volume.apfs_container_uuid = Some("APFS-CONTAINER-OLD".to_string());
        previous_volume.apfs_role = Some(ApfsVolumeRole::Data);
        let mut current_volume = previous_volume.clone();
        current_volume.apfs_container_uuid = Some("APFS-CONTAINER-NEW".to_string());
        current_volume.apfs_role = Some(ApfsVolumeRole::System);
        let previous = VolumeDiscoveryReport {
            volumes: vec![previous_volume],
        };
        let current = VolumeDiscoveryReport {
            volumes: vec![current_volume],
        };

        let diff = VolumeTopologyDiff::evaluate(&previous, &current);

        assert_eq!(diff.changes.len(), 1);
        assert_eq!(diff.changes[0].reason, "volume-apfs-metadata-changed");
        assert!(diff.changes[0].invalidate_sidebar);
        assert!(diff.changes[0].invalidate_operation_policy);
        assert!(diff.changes[0].invalidate_index_admission);
        assert!(diff.changes[0].rescan_index);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn topology_diff_reports_removable_media_truth_changes() {
        let root = unique_temp_dir("gfm-volume-topology-removable-media");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();
        let mut previous_volume = VolumeDescriptor::for_path(&root).unwrap();
        previous_volume.removable = false;
        previous_volume.ejectable = true;
        let mut current_volume = previous_volume.clone();
        current_volume.removable = true;
        let previous = VolumeDiscoveryReport {
            volumes: vec![previous_volume],
        };
        let current = VolumeDiscoveryReport {
            volumes: vec![current_volume],
        };

        let diff = VolumeTopologyDiff::evaluate(&previous, &current);

        assert_eq!(diff.changes.len(), 1);
        assert_eq!(diff.changes[0].reason, "volume-media-truth-changed");
        assert!(diff.changes[0].invalidate_sidebar);
        assert!(diff.changes[0].invalidate_operation_policy);
        assert!(diff.changes[0].invalidate_index_admission);
        assert!(diff.changes[0].rescan_index);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn topology_diff_reports_native_mount_table_trait_changes() {
        let root = unique_temp_dir("gfm-volume-topology-mount-table");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();
        let mut previous_volume = VolumeDescriptor::for_path(&root).unwrap();
        previous_volume.mount_filesystem = Some("apfs".to_string());
        previous_volume.mount_flags = Some(0);
        previous_volume.mount_local = Some(true);
        previous_volume.mount_read_only = Some(false);
        let mut current_volume = previous_volume.clone();
        current_volume.mount_flags = Some(0x0000_1000);
        current_volume.mount_read_only = Some(true);
        let previous = VolumeDiscoveryReport {
            volumes: vec![previous_volume],
        };
        let current = VolumeDiscoveryReport {
            volumes: vec![current_volume],
        };

        let diff = VolumeTopologyDiff::evaluate(&previous, &current);

        assert_eq!(diff.changes.len(), 1);
        assert_eq!(diff.changes[0].reason, "volume-mount-table-changed");
        assert!(diff.changes[0].invalidate_sidebar);
        assert!(diff.changes[0].invalidate_operation_policy);
        assert!(diff.changes[0].invalidate_index_admission);
        assert!(diff.changes[0].rescan_index);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn topology_diff_reports_case_sensitivity_as_index_semantics_change() {
        let root = unique_temp_dir("gfm-volume-topology-case-sensitivity");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();
        let mut previous_volume = VolumeDescriptor::for_path(&root).unwrap();
        previous_volume.case_sensitive = Some(false);
        let mut current_volume = previous_volume.clone();
        current_volume.case_sensitive = Some(true);
        let previous = VolumeDiscoveryReport {
            volumes: vec![previous_volume],
        };
        let current = VolumeDiscoveryReport {
            volumes: vec![current_volume],
        };

        let diff = VolumeTopologyDiff::evaluate(&previous, &current);

        assert_eq!(diff.changes.len(), 1);
        assert_eq!(diff.changes[0].reason, "volume-case-sensitivity-changed");
        assert_eq!(diff.changes[0].previous_case_sensitive, Some(false));
        assert_eq!(diff.changes[0].current_case_sensitive, Some(true));
        assert!(diff.changes[0].invalidate_sidebar);
        assert!(diff.changes[0].invalidate_operation_policy);
        assert!(diff.changes[0].invalidate_index_admission);
        assert!(diff.changes[0].rescan_index);
        assert!(diff
            .as_tsv()
            .contains("\tprevious-case-sensitive=false\tcurrent-case-sensitive=true\t"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn topology_diff_reports_native_api_status_as_policy_change() {
        let root = unique_temp_dir("gfm-volume-topology-api-status");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();
        let mut previous_volume = VolumeDescriptor::for_path(&root).unwrap();
        previous_volume.native_status = Some(NativeVolumeStatus::Unavailable);
        previous_volume.resource_status = Some(NativeVolumeStatus::Unavailable);
        previous_volume.mount_table_status = Some(NativeVolumeStatus::Unavailable);
        let mut current_volume = previous_volume.clone();
        current_volume.native_status = Some(NativeVolumeStatus::Available);
        current_volume.resource_status = Some(NativeVolumeStatus::Available);
        current_volume.mount_table_status = Some(NativeVolumeStatus::Available);
        let previous = VolumeDiscoveryReport {
            volumes: vec![previous_volume],
        };
        let current = VolumeDiscoveryReport {
            volumes: vec![current_volume],
        };

        let diff = VolumeTopologyDiff::evaluate(&previous, &current);

        assert_eq!(diff.changes.len(), 1);
        assert_eq!(diff.changes[0].reason, "volume-api-status-changed");
        assert_eq!(
            diff.changes[0].previous_native_status,
            Some(NativeVolumeStatus::Unavailable)
        );
        assert_eq!(
            diff.changes[0].current_native_status,
            Some(NativeVolumeStatus::Available)
        );
        assert!(diff.changes[0].invalidate_sidebar);
        assert!(diff.changes[0].invalidate_operation_policy);
        assert!(diff.changes[0].invalidate_index_admission);
        assert!(diff.changes[0].rescan_index);
        assert!(diff
            .as_tsv()
            .contains("\tprevious-native-status=unavailable\tcurrent-native-status=available\t"));
        assert!(diff.as_tsv().contains(
            "\tprevious-resource-status=unavailable\tcurrent-resource-status=available\t"
        ));
        assert!(diff
            .as_tsv()
            .contains("\tprevious-mount-status=unavailable\tcurrent-mount-status=available\t"));

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
    fn volume_event_transition_keeps_label_only_change_sidebar_scoped() {
        let root = unique_temp_dir("gfm-volume-event-label-change");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();
        let previous = VolumeDescriptor::for_path(&root).unwrap();
        let mut current = previous.clone();
        current.label = "Renamed Drive".to_string();

        let report = VolumeEventInvalidationReport::from_transition(
            VolumeEventKind::DescriptionChanged,
            NativeVolumeStatus::Available,
            Some(&previous),
            Some(&current),
            None,
        );

        assert_eq!(report.reason, "volume-label-changed");
        assert_eq!(report.previous_kind, Some(VolumeKind::External));
        assert_eq!(report.current_kind, Some(VolumeKind::External));
        assert_eq!(report.previous_mount_state, Some(MountState::Mounted));
        assert_eq!(report.current_mount_state, Some(MountState::Mounted));
        assert!(report.invalidate_sidebar);
        assert!(!report.invalidate_operation_policy);
        assert!(!report.invalidate_index_admission);
        assert!(!report.rescan_index);
        assert!(report.as_tsv().contains(
            "\tsidebar=true\toperation-policy=false\tindex-admission=false\trescan-index=false\t"
        ));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn volume_event_transition_keeps_reachability_change_hot_path_visible() {
        let root = unique_temp_dir("gfm-volume-event-reachability-change");
        fs::write(root.join(VOLUME_MARKER), "network-smb\n").unwrap();
        let previous = VolumeDescriptor::for_path(&root).unwrap();
        let mut current = previous.clone();
        current.reachable = Some(false);

        let report = VolumeEventInvalidationReport::from_transition(
            VolumeEventKind::DescriptionChanged,
            NativeVolumeStatus::Available,
            Some(&previous),
            Some(&current),
            None,
        );

        assert_eq!(report.reason, "volume-locality-changed");
        assert_eq!(report.previous_kind, Some(VolumeKind::Network));
        assert_eq!(report.current_kind, Some(VolumeKind::Network));
        assert!(report.invalidate_sidebar);
        assert!(report.invalidate_operation_policy);
        assert!(report.invalidate_index_admission);
        assert!(report.rescan_index);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn volume_event_transition_reports_case_sensitivity_changes() {
        let root = unique_temp_dir("gfm-volume-event-case-sensitivity-change");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();
        let mut previous = VolumeDescriptor::for_path(&root).unwrap();
        previous.case_sensitive = Some(false);
        previous.native_status = Some(NativeVolumeStatus::Available);
        previous.resource_status = Some(NativeVolumeStatus::Available);
        previous.mount_table_status = Some(NativeVolumeStatus::Available);
        let mut current = previous.clone();
        current.case_sensitive = Some(true);

        let report = VolumeEventInvalidationReport::from_transition(
            VolumeEventKind::DescriptionChanged,
            NativeVolumeStatus::Available,
            Some(&previous),
            Some(&current),
            None,
        );

        assert_eq!(report.reason, "volume-case-sensitivity-changed");
        assert_eq!(report.previous_case_sensitive, Some(false));
        assert_eq!(report.current_case_sensitive, Some(true));
        assert!(report.invalidate_sidebar);
        assert!(report.invalidate_operation_policy);
        assert!(report.invalidate_index_admission);
        assert!(report.rescan_index);
        assert!(report
            .as_tsv()
            .contains("\tprevious-case-sensitive=false\t"));
        assert!(report
            .as_tsv()
            .contains("\tprevious-native-status=available\t"));
        assert!(report.as_tsv().contains("\tcurrent-case-sensitive=true\t"));
        assert!(report
            .as_tsv()
            .contains("\tcurrent-native-status=available\t"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn volume_event_transition_reports_removable_media_truth_changes() {
        let root = unique_temp_dir("gfm-volume-event-removable-media-change");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();
        let mut previous = VolumeDescriptor::for_path(&root).unwrap();
        previous.removable = false;
        previous.ejectable = true;
        previous.native_status = Some(NativeVolumeStatus::Available);
        previous.resource_status = Some(NativeVolumeStatus::Available);
        previous.mount_table_status = Some(NativeVolumeStatus::Available);
        let mut current = previous.clone();
        current.removable = true;

        let report = VolumeEventInvalidationReport::from_transition(
            VolumeEventKind::DescriptionChanged,
            NativeVolumeStatus::Available,
            Some(&previous),
            Some(&current),
            None,
        );

        assert_eq!(report.reason, "volume-media-truth-changed");
        assert_eq!(report.previous_kind, Some(VolumeKind::External));
        assert_eq!(report.current_kind, Some(VolumeKind::External));
        assert!(report.invalidate_sidebar);
        assert!(report.invalidate_operation_policy);
        assert!(report.invalidate_index_admission);
        assert!(report.rescan_index);
        assert!(report.as_tsv().contains(
            "\tsidebar=true\toperation-policy=true\tindex-admission=true\trescan-index=true\t"
        ));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn volume_event_transition_reports_api_status_changes() {
        let root = unique_temp_dir("gfm-volume-event-api-status-change");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();
        let mut previous = VolumeDescriptor::for_path(&root).unwrap();
        previous.native_status = Some(NativeVolumeStatus::Unavailable);
        previous.resource_status = Some(NativeVolumeStatus::Unavailable);
        previous.mount_table_status = Some(NativeVolumeStatus::Unavailable);
        let mut current = previous.clone();
        current.native_status = Some(NativeVolumeStatus::Available);
        current.resource_status = Some(NativeVolumeStatus::Available);
        current.mount_table_status = Some(NativeVolumeStatus::Available);

        let report = VolumeEventInvalidationReport::from_transition(
            VolumeEventKind::DescriptionChanged,
            NativeVolumeStatus::Available,
            Some(&previous),
            Some(&current),
            None,
        );

        assert_eq!(report.reason, "volume-api-status-changed");
        assert_eq!(
            report.previous_native_status,
            Some(NativeVolumeStatus::Unavailable)
        );
        assert_eq!(
            report.current_native_status,
            Some(NativeVolumeStatus::Available)
        );
        assert!(report.invalidate_operation_policy);
        assert!(report.invalidate_index_admission);
        assert!(report
            .as_tsv()
            .contains("\tprevious-native-status=unavailable\t"));
        assert!(report.as_tsv().contains("\tcurrent-kind=external\t"));
        assert!(report.as_tsv().contains(
            "\tcurrent-native-status=available\tcurrent-resource-status=available\tcurrent-mount-status=available\t"
        ));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn volume_event_state_adds_appeared_volume_and_reports_current_transition() {
        let root = unique_temp_dir("gfm-volume-event-state-appeared");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();
        let descriptor = VolumeDescriptor::for_path(&root).unwrap();
        let mut state = VolumeEventState::new(VolumeDiscoveryReport {
            volumes: Vec::new(),
        });

        let transition = state.apply_parts_transition(
            VolumeEventKind::Appeared,
            NativeVolumeStatus::Available,
            Some(root.clone()),
            Some(descriptor.clone()),
            None,
        );

        assert_eq!(transition.previous, None);
        assert_eq!(transition.current.as_ref(), Some(&descriptor));
        assert_eq!(
            transition.invalidation.current_kind,
            Some(VolumeKind::External)
        );
        assert_eq!(state.report().volumes, vec![descriptor]);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn volume_event_state_updates_existing_volume_by_stable_identity() {
        let root = unique_temp_dir("gfm-volume-event-state-description");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();
        let previous = VolumeDescriptor::for_path(&root).unwrap();
        let mut current = previous.clone();
        current.label = "Renamed Event Volume".to_string();
        let mut state = VolumeEventState::new(VolumeDiscoveryReport {
            volumes: vec![previous.clone()],
        });

        let transition = state.apply_parts_transition(
            VolumeEventKind::DescriptionChanged,
            NativeVolumeStatus::Available,
            Some(root.clone()),
            Some(current.clone()),
            None,
        );

        assert_eq!(transition.previous.as_ref(), Some(&previous));
        assert_eq!(transition.current.as_ref(), Some(&current));
        assert_eq!(transition.invalidation.reason, "volume-label-changed");
        assert_eq!(state.report().volumes, vec![current]);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn volume_event_state_updates_path_index_after_stable_identity_move() {
        let root = unique_temp_dir("gfm-volume-event-state-path-index");
        let old_path = root.join("Old Mount");
        let new_path = root.join("New Mount");
        fs::create_dir_all(&old_path).unwrap();
        fs::create_dir_all(&new_path).unwrap();
        fs::write(old_path.join(VOLUME_MARKER), "external-removable\n").unwrap();
        let previous = VolumeDescriptor::for_path(&old_path).unwrap();
        let mut current = previous.clone();
        current.path = new_path.clone();
        current.label = "New Mount".to_string();
        let mut state = VolumeEventState::new(VolumeDiscoveryReport {
            volumes: vec![previous.clone()],
        });

        let changed = state.apply_parts_transition(
            VolumeEventKind::DescriptionChanged,
            NativeVolumeStatus::Available,
            Some(new_path.clone()),
            Some(current.clone()),
            None,
        );

        assert_eq!(changed.previous.as_ref(), Some(&previous));
        assert_eq!(state.report().volumes, vec![current.clone()]);

        let stale_remove = state.apply_parts_transition(
            VolumeEventKind::Disappeared,
            NativeVolumeStatus::Available,
            Some(old_path),
            None,
            None,
        );

        assert!(stale_remove.previous.is_none());
        assert_eq!(state.report().volumes, vec![current.clone()]);

        let current_remove = state.apply_parts_transition(
            VolumeEventKind::Disappeared,
            NativeVolumeStatus::Available,
            Some(new_path),
            None,
            None,
        );

        assert_eq!(current_remove.previous.as_ref(), Some(&current));
        assert!(state.report().volumes.is_empty());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn volume_event_state_replaces_path_when_stable_identity_changes() {
        let root = unique_temp_dir("gfm-volume-event-state-stable-index");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();
        let previous = VolumeDescriptor::for_path(&root).unwrap();
        let mut current = previous.clone();
        current.stable_identity = format!("{}-replacement", previous.stable_identity);
        let mut state = VolumeEventState::new(VolumeDiscoveryReport {
            volumes: vec![previous.clone()],
        });

        let changed = state.apply_parts_transition(
            VolumeEventKind::DescriptionChanged,
            NativeVolumeStatus::Available,
            Some(root.clone()),
            Some(current.clone()),
            None,
        );

        assert_eq!(changed.previous.as_ref(), Some(&previous));
        assert_eq!(state.report().volumes, vec![current.clone()]);

        let removed = state.apply_parts_transition(
            VolumeEventKind::Disappeared,
            NativeVolumeStatus::Available,
            Some(root.clone()),
            None,
            None,
        );

        assert_eq!(removed.previous.as_ref(), Some(&current));
        assert!(state.report().volumes.is_empty());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn volume_event_state_removes_disappeared_volume_by_path() {
        let root = unique_temp_dir("gfm-volume-event-state-disappeared");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();
        let previous = VolumeDescriptor::for_path(&root).unwrap();
        let mut state = VolumeEventState::new(VolumeDiscoveryReport {
            volumes: vec![previous.clone()],
        });

        let transition = state.apply_parts_transition(
            VolumeEventKind::Disappeared,
            NativeVolumeStatus::Available,
            Some(root.clone()),
            None,
            None,
        );

        assert_eq!(transition.previous.as_ref(), Some(&previous));
        assert_eq!(transition.current, None);
        assert_eq!(
            transition.invalidation.current_mount_state,
            Some(MountState::Unmounted)
        );
        assert!(state.report().volumes.is_empty());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn volume_event_state_keeps_snapshot_on_unavailable_event() {
        let root = unique_temp_dir("gfm-volume-event-state-unavailable");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();
        let previous = VolumeDescriptor::for_path(&root).unwrap();
        let mut state = VolumeEventState::new(VolumeDiscoveryReport {
            volumes: vec![previous.clone()],
        });

        let transition = state.apply_parts_transition(
            VolumeEventKind::Unavailable,
            NativeVolumeStatus::Unavailable,
            None,
            None,
            Some("diskarbitration-event-session-unavailable".to_string()),
        );

        assert_eq!(transition.previous, None);
        assert_eq!(transition.current, None);
        assert_eq!(
            transition.invalidation.reason,
            "diskarbitration-event-session-unavailable"
        );
        assert_eq!(state.report().volumes, vec![previous]);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn disappeared_volume_event_reports_previous_volume_and_unmounted_current_state() {
        let root = unique_temp_dir("gfm-volume-event-disappeared");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();
        let mut descriptor = VolumeDescriptor::for_path(&root).unwrap();
        descriptor.native_status = Some(NativeVolumeStatus::Available);
        descriptor.resource_status = Some(NativeVolumeStatus::Available);
        descriptor.mount_table_status = Some(NativeVolumeStatus::Available);

        let report = VolumeEventInvalidationReport::from_parts(
            VolumeEventKind::Disappeared,
            NativeVolumeStatus::Available,
            Some(root.clone()),
            Some(&descriptor),
            None,
        );

        assert_eq!(report.previous_kind, Some(VolumeKind::External));
        assert_eq!(report.previous_mount_state, Some(MountState::Mounted));
        assert_eq!(
            report.previous_native_status,
            Some(NativeVolumeStatus::Available)
        );
        assert_eq!(report.current_kind, None);
        assert_eq!(report.current_mount_state, Some(MountState::Unmounted));
        assert_eq!(report.current_native_status, None);
        assert!(report.invalidate_sidebar);
        assert!(report.invalidate_operation_policy);
        assert!(report.invalidate_index_admission);
        assert!(report.rescan_index);
        assert!(report
            .as_tsv()
            .contains("\tprevious-kind=external\tprevious-mount=mounted\t"));
        assert!(report
            .as_tsv()
            .contains("\tcurrent-kind=-\tcurrent-mount=unmounted\t"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_disappeared_event_for_missing_path_does_not_synthesize_descriptor() {
        let root = std::env::temp_dir().join(format!(
            "gfm-native-volume-event-missing-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&root);
        let mut description = native_description(|description| {
            description.status = NativeVolumeStatus::Missing;
            description.volume_path = Some(root.clone());
            description.reason = Some("volume path does not exist".to_string());
        });
        description.volume_name = Some("Gone Drive".to_string());
        let event = gfm_mac_sys::NativeVolumeEvent {
            kind: gfm_mac_sys::NativeVolumeEventKind::Disappeared,
            description,
        };

        let report = VolumeEventReport::from_native(event);
        let invalidation = VolumeEventInvalidationReport::from_event(&report);

        assert_eq!(report.kind, VolumeEventKind::Disappeared);
        assert_eq!(report.native_status, NativeVolumeStatus::Missing);
        assert_eq!(report.path.as_deref(), Some(root.as_path()));
        assert!(report.descriptor.is_none());
        assert_eq!(
            invalidation.current_mount_state,
            Some(MountState::Unmounted)
        );
        assert!(invalidation.invalidate_sidebar);
        assert!(invalidation.invalidate_operation_policy);
        assert!(invalidation.invalidate_index_admission);
    }

    #[test]
    fn native_unavailable_event_does_not_publish_current_descriptor() {
        let root = unique_temp_dir("gfm-native-volume-event-unavailable");
        fs::write(root.join(VOLUME_MARKER), "network-smb\n").unwrap();
        let description = native_description(|description| {
            description.status = NativeVolumeStatus::Unavailable;
            description.volume_path = Some(root.clone());
            description.reason = Some("diskarbitration session unavailable".to_string());
        });
        let event = gfm_mac_sys::NativeVolumeEvent {
            kind: gfm_mac_sys::NativeVolumeEventKind::Unavailable,
            description,
        };

        let report = VolumeEventReport::from_native(event);
        let invalidation = VolumeEventInvalidationReport::from_event(&report);

        assert_eq!(report.kind, VolumeEventKind::Unavailable);
        assert_eq!(report.native_status, NativeVolumeStatus::Unavailable);
        assert_eq!(report.path.as_deref(), Some(root.as_path()));
        assert!(report.descriptor.is_none());
        assert_eq!(invalidation.previous_kind, None);
        assert_eq!(invalidation.current_kind, None);
        assert!(invalidation.invalidate_sidebar);
        assert!(invalidation.invalidate_operation_policy);
        assert!(invalidation.invalidate_index_admission);

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
    fn description_changed_volume_event_preserves_native_probe_failure_reason() {
        let report = VolumeEventInvalidationReport::from_parts(
            VolumeEventKind::DescriptionChanged,
            NativeVolumeStatus::Unavailable,
            Some(PathBuf::from("/Volumes/Unprobeable")),
            None,
            Some("volume path state unavailable: permission denied".to_string()),
        );

        assert_eq!(
            report.reason,
            "volume path state unavailable: permission denied"
        );
        assert!(report.invalidate_sidebar);
        assert!(report.invalidate_operation_policy);
        assert!(report.invalidate_index_admission);
        assert!(report.rescan_index);
        assert!(report
            .as_tsv()
            .contains("\treason=volume path state unavailable: permission denied"));
    }

    #[test]
    fn unavailable_volume_transition_preserves_previous_descriptor_as_previous_state() {
        let root = unique_temp_dir("gfm-volume-event-unavailable-transition");
        fs::write(root.join(VOLUME_MARKER), "network-smb\n").unwrap();
        let previous = VolumeDescriptor::for_path(&root).unwrap();

        let report = VolumeEventInvalidationReport::from_transition(
            VolumeEventKind::Unavailable,
            NativeVolumeStatus::Unavailable,
            Some(&previous),
            None,
            Some("diskarbitration-description-unavailable".to_string()),
        );

        assert_eq!(report.path, Some(root.clone()));
        assert_eq!(report.previous_kind, Some(VolumeKind::Network));
        assert_eq!(report.previous_mount_state, Some(MountState::Mounted));
        assert_eq!(report.current_kind, None);
        assert_eq!(report.current_mount_state, None);
        assert!(report.invalidate_sidebar);
        assert!(report.invalidate_operation_policy);
        assert!(report.invalidate_index_admission);
        assert!(report.rescan_index);
        assert_eq!(report.reason, "diskarbitration-description-unavailable");
        let tsv = report.as_tsv();
        assert!(tsv.contains("\tprevious-kind=network\t"));
        assert!(tsv.contains("\tprevious-mount=mounted\t"));
        assert!(tsv.contains("\tcurrent-kind=-\t"));
        assert!(tsv.contains("\tcurrent-mount=-\t"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn volume_event_state_preserves_previous_descriptor_for_missing_disappearance() {
        let root = unique_temp_dir("gfm-volume-event-state-disappeared");
        fs::write(root.join(VOLUME_MARKER), "network-smb\n").unwrap();
        let previous = VolumeDescriptor::for_path(&root).unwrap();
        let mut state = VolumeEventState::new(VolumeDiscoveryReport {
            volumes: vec![previous.clone()],
        });
        fs::remove_dir_all(&root).unwrap();

        let transition = state.apply_parts_transition(
            VolumeEventKind::Disappeared,
            NativeVolumeStatus::Missing,
            Some(root.clone()),
            None,
            None,
        );

        assert_eq!(transition.previous.as_ref(), Some(&previous));
        assert!(transition.current.is_none());
        assert_eq!(
            transition.invalidation.previous_kind,
            Some(VolumeKind::Network)
        );
        assert_eq!(
            transition.invalidation.previous_mount_state,
            Some(MountState::Mounted)
        );
        assert_eq!(
            transition.invalidation.current_mount_state,
            Some(MountState::Unmounted)
        );
        assert!(transition.invalidation.invalidate_index_admission);
        assert!(state.report().volumes.is_empty());
    }

    #[test]
    fn volume_event_state_updates_current_descriptor_by_stable_identity() {
        let root = unique_temp_dir("gfm-volume-event-state-changed");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();
        let previous = VolumeDescriptor::for_path(&root).unwrap();
        let mut current = previous.clone();
        current.reachable = Some(false);
        let mut state = VolumeEventState::new(VolumeDiscoveryReport {
            volumes: vec![previous],
        });

        let transition = state.apply_parts_transition(
            VolumeEventKind::DescriptionChanged,
            NativeVolumeStatus::Available,
            Some(root.clone()),
            Some(current.clone()),
            None,
        );

        assert_eq!(transition.invalidation.reason, "volume-locality-changed");
        assert_eq!(state.report().volumes, vec![current]);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unchanged_description_transition_keeps_status_reason_sidebar_scoped() {
        let root = unique_temp_dir("gfm-volume-event-state-unchanged-native-reason");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();
        let mut descriptor = VolumeDescriptor::for_path(&root).unwrap();
        descriptor.native_status = Some(NativeVolumeStatus::Unavailable);
        let mut state = VolumeEventState::new(VolumeDiscoveryReport {
            volumes: vec![descriptor.clone()],
        });

        let transition = state.apply_parts_transition(
            VolumeEventKind::DescriptionChanged,
            NativeVolumeStatus::Unavailable,
            Some(root.clone()),
            Some(descriptor),
            Some("diskarbitration-volume-unavailable".to_string()),
        );

        assert_eq!(
            transition.invalidation.reason,
            "volume-event-description-changed"
        );
        assert!(transition.invalidation.invalidate_sidebar);
        assert!(!transition.invalidation.invalidate_operation_policy);
        assert!(!transition.invalidation.invalidate_index_admission);
        assert!(!transition.invalidation.rescan_index);

        fs::remove_dir_all(root).unwrap();
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
    fn volume_operation_reports_missing_path_without_native_submission() {
        let report = VolumeOperationReport::execute(
            "/tmp/gfm-volume-operation-missing",
            VolumeOperation::Eject,
        )
        .unwrap();

        assert_eq!(report.disposition, VolumeOperationDisposition::Missing);
        assert_eq!(report.native_status, None);
        assert_eq!(report.reason, "volume-path-missing");
        assert!(report.as_tsv().contains("\tdisposition=missing\t"));
    }

    #[cfg(unix)]
    #[test]
    fn volume_operation_surfaces_path_probe_errors_as_unavailable() {
        let path = invalid_path("gfm-volume-operation-invalid");

        let report = VolumeOperationReport::execute(&path, VolumeOperation::Eject).unwrap();

        assert_eq!(report.disposition, VolumeOperationDisposition::Unavailable);
        assert_eq!(report.native_status, None);
        assert_eq!(report.volume, None);
        assert!(report.reason.contains("volume-path-existence-unavailable"));
        assert!(report.as_tsv().contains("\tdisposition=unavailable\t"));
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
            disposition_for_native_operation(NativeVolumeOperationStatus::Error),
            VolumeOperationDisposition::Failed
        );
        assert_eq!(
            disposition_for_native_operation(NativeVolumeOperationStatus::ExclusiveAccess),
            VolumeOperationDisposition::Busy
        );
        assert_eq!(
            disposition_for_native_operation(NativeVolumeOperationStatus::NoResources),
            VolumeOperationDisposition::Busy
        );
        assert_eq!(
            disposition_for_native_operation(NativeVolumeOperationStatus::NotReady),
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
            disposition_for_native_operation(NativeVolumeOperationStatus::Cancelled),
            VolumeOperationDisposition::Cancelled
        );
        assert_eq!(
            disposition_for_native_operation(NativeVolumeOperationStatus::Unavailable),
            VolumeOperationDisposition::Unavailable
        );
        assert_eq!(
            disposition_for_native_operation(NativeVolumeOperationStatus::NotMounted),
            VolumeOperationDisposition::Refused
        );
        assert_eq!(
            disposition_for_native_operation(NativeVolumeOperationStatus::BadArgument),
            VolumeOperationDisposition::Refused
        );
        assert_eq!(
            disposition_for_native_operation(NativeVolumeOperationStatus::NotWritable),
            VolumeOperationDisposition::Refused
        );
        assert_eq!(
            disposition_for_native_operation(NativeVolumeOperationStatus::NotFound),
            VolumeOperationDisposition::Missing
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

    fn private_var_alias_for(path: &Path) -> Option<PathBuf> {
        path.strip_prefix("/private")
            .ok()
            .map(|stripped| Path::new("/").join(stripped))
    }

    fn resource_values(
        configure: impl FnOnce(&mut NativeVolumeResourceValues),
    ) -> NativeVolumeResourceValues {
        let mut values = NativeVolumeResourceValues {
            status: NativeVolumeStatus::Available,
            is_automounted: None,
            is_browsable: None,
            is_ejectable: None,
            is_encrypted: None,
            is_internal: None,
            is_local: None,
            is_read_only: None,
            is_reachable: None,
            is_removable: None,
            is_root_file_system: None,
            remount_url: None,
            supports_case_preserved_names: None,
            supports_case_sensitive_names: None,
            supports_file_cloning: None,
            supports_hard_links: None,
            supports_sparse_files: None,
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
            whole_disk_media_uuid: None,
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

    #[cfg(unix)]
    fn invalid_path(prefix: &str) -> PathBuf {
        let mut bytes = std::env::temp_dir().into_os_string().into_vec();
        bytes.push(b'/');
        bytes.extend_from_slice(prefix.as_bytes());
        bytes.push(0);
        PathBuf::from(OsString::from_vec(bytes))
    }
}
