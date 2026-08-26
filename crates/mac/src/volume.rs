use gfm_mac_sys::{NativeVolumeDescription, NativeVolumeStatus};
use gfm_types::{GfmError, Result, VolumeId};
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
    pub ejectable: bool,
    pub writable: bool,
    pub read_only: bool,
    pub mountable: Option<bool>,
    pub capacity: VolumeCapacity,
    pub commands: VolumeCommandPolicy,
    pub native_status: Option<NativeVolumeStatus>,
    pub bsd_name: Option<String>,
    pub volume_uuid: Option<String>,
    pub media_uuid: Option<String>,
    pub filesystem: Option<String>,
    pub media_content: Option<String>,
    pub device_protocol: Option<String>,
    pub device_model: Option<String>,
    pub device_vendor: Option<String>,
    pub source: String,
}

impl VolumeDescriptor {
    pub fn for_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let metadata = fs::metadata(&path).map_err(|err| GfmError::io(&path, err))?;
        let id = volume_id(&metadata);
        let marker = marker_kind(&path);
        let native = marker
            .is_none()
            .then(|| gfm_mac_sys::copy_volume_description_for_path(&path));
        let native_status = native.as_ref().map(|native| native.status);
        let label = native
            .as_ref()
            .and_then(|native| native.volume_name.clone())
            .unwrap_or_else(|| volume_label(&path));
        let kind = classify_volume(&path, marker.as_deref(), native.as_ref());
        let mount_state = if path.exists() {
            MountState::Mounted
        } else {
            MountState::Stale
        };
        let removable = native
            .as_ref()
            .and_then(|native| native.media_removable)
            .unwrap_or({
                matches!(
                    kind,
                    VolumeKind::External | VolumeKind::Removable | VolumeKind::DiskImage
                )
            });
        let network = native
            .as_ref()
            .and_then(|native| native.volume_network)
            .unwrap_or(kind == VolumeKind::Network);
        let ejectable = native
            .as_ref()
            .and_then(|native| native.media_ejectable)
            .unwrap_or(removable || network);
        let writable = native
            .as_ref()
            .and_then(|native| native.media_writable)
            .unwrap_or_else(|| !metadata.permissions().readonly());
        let read_only = !writable;
        let mountable = native.as_ref().and_then(|native| native.volume_mountable);
        let capacity = VolumeCapacity::read(&path);
        let commands = command_policy(kind, mount_state, ejectable);
        let stable_identity = stable_identity(id, &path, native.as_ref());
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
            ejectable,
            writable,
            read_only,
            mountable,
            capacity,
            commands,
            native_status,
            bsd_name: native
                .as_ref()
                .and_then(|native| native.media_bsd_name.clone()),
            volume_uuid: native
                .as_ref()
                .and_then(|native| native.volume_uuid.clone()),
            media_uuid: native.as_ref().and_then(|native| native.media_uuid.clone()),
            filesystem: native.as_ref().and_then(|native| {
                native
                    .volume_kind
                    .clone()
                    .or_else(|| native.volume_type.clone())
            }),
            media_content: native
                .as_ref()
                .and_then(|native| native.media_content.clone()),
            device_protocol: native
                .as_ref()
                .and_then(|native| native.device_protocol.clone()),
            device_model: native
                .as_ref()
                .and_then(|native| native.device_model.clone()),
            device_vendor: native
                .as_ref()
                .and_then(|native| native.device_vendor.clone()),
            source,
        })
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "volume\t{}\t{}\tpath={}\tkind={}\tmount={}\tremovable={}\tnetwork={}\tejectable={}\ttotal={}\tavailable={}\teject={}\tmount={}\tunmount={}\tsource={}\treason={}\tstable-id={}\tnative-status={}\twritable={}\tread-only={}\tmountable={}\tbsd={}\tvolume-uuid={}\tmedia-uuid={}\tfs={}\tmedia-content={}\tprotocol={}\tmodel={}\tvendor={}",
            self.id.0,
            escape_field(&self.label),
            self.path.display(),
            self.kind.as_str(),
            self.mount_state.as_str(),
            self.removable,
            self.network,
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
        let mut paths = vec![PathBuf::from("/")];
        if let Ok(entries) = fs::read_dir("/Volumes") {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    paths.push(path);
                }
            }
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

    pub fn as_tsv(&self) -> String {
        let mut lines = vec![format!("volumes\tcount={}", self.volumes.len())];
        lines.extend(self.volumes.iter().map(VolumeDescriptor::as_tsv));
        lines.join("\n")
    }
}

fn classify_volume(
    path: &Path,
    marker: Option<&str>,
    native: Option<&gfm_mac_sys::NativeVolumeDescription>,
) -> VolumeKind {
    match marker {
        Some("network") | Some("network-smb") | Some("network-afp") | Some("network-nfs") => {
            return VolumeKind::Network;
        }
        Some("external") | Some("external-removable") => return VolumeKind::External,
        Some("removable") => return VolumeKind::Removable,
        Some("disk-image") => return VolumeKind::DiskImage,
        Some("system") => return VolumeKind::System,
        Some("internal") => return VolumeKind::Internal,
        _ => {}
    }

    if let Some(kind) = classify_native_volume(path, native) {
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
) -> Option<VolumeKind> {
    let native = native.filter(|native| native.status == NativeVolumeStatus::Available)?;
    if path == Path::new("/") {
        return Some(VolumeKind::System);
    }
    if native.volume_network == Some(true) {
        return Some(VolumeKind::Network);
    }

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

fn stable_identity(id: VolumeId, path: &Path, native: Option<&NativeVolumeDescription>) -> String {
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
    format!("dev:{}:{}", id.0, escape_field(&path.display().to_string()))
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
        assert_eq!(descriptor.read_only, !descriptor.writable);
        assert!(descriptor.stable_identity.starts_with("diskarbitration:"));
        assert_eq!(descriptor.commands.eject, VolumeCommandState::Hidden);
        assert!(descriptor.capacity.total_bytes > 0);
        assert!(descriptor.as_tsv().contains("\tnative-status=available\t"));
        assert!(descriptor.as_tsv().contains("\tstable-id="));
        assert!(descriptor.as_tsv().contains("\tread-only="));
    }

    #[test]
    fn classifies_external_marker_as_ejectable() {
        let root = unique_temp_dir("gfm-volume-external");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();

        let descriptor = VolumeDescriptor::for_path(&root).unwrap();

        assert_eq!(descriptor.kind, VolumeKind::External);
        assert!(descriptor.removable);
        assert!(descriptor.ejectable);
        assert_eq!(descriptor.native_status, None);
        assert!(descriptor.stable_identity.starts_with("dev:"));
        assert_eq!(descriptor.commands.eject, VolumeCommandState::Enabled);
        assert!(descriptor
            .as_tsv()
            .contains("source=fixture-marker:external-removable"));

        fs::remove_dir_all(root).unwrap();
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
}
