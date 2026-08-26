use gfm_mac_sys::{
    NativeVolumeDescription, NativeVolumeOperation, NativeVolumeOperationStatus,
    NativeVolumeResourceValues, NativeVolumeStatus,
};
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
    pub case_sensitive: Option<bool>,
    pub case_preserving: Option<bool>,
    pub local: Option<bool>,
    pub internal: Option<bool>,
    pub mountable: Option<bool>,
    pub capacity: VolumeCapacity,
    pub commands: VolumeCommandPolicy,
    pub native_status: Option<NativeVolumeStatus>,
    pub resource_status: Option<NativeVolumeStatus>,
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
        let resource = marker
            .is_none()
            .then(|| gfm_mac_sys::copy_volume_resource_values(&path));
        let native_status = native.as_ref().map(|native| native.status);
        let resource_status = resource.as_ref().map(|resource| resource.status);
        let label = native
            .as_ref()
            .and_then(|native| native.volume_name.clone())
            .unwrap_or_else(|| volume_label(&path));
        let kind = classify_volume(&path, marker.as_deref(), native.as_ref(), resource.as_ref());
        let mount_state = if path.exists() {
            MountState::Mounted
        } else {
            MountState::Stale
        };
        let removable = resource
            .as_ref()
            .and_then(|resource| resource.is_removable)
            .or_else(|| native.as_ref().and_then(|native| native.media_removable))
            .unwrap_or({
                matches!(
                    kind,
                    VolumeKind::External | VolumeKind::Removable | VolumeKind::DiskImage
                )
            });
        let local = resource.as_ref().and_then(|resource| resource.is_local);
        let network = local
            .map(|local| !local)
            .or_else(|| native.as_ref().and_then(|native| native.volume_network))
            .unwrap_or(kind == VolumeKind::Network);
        let ejectable = resource
            .as_ref()
            .and_then(|resource| resource.is_ejectable)
            .or_else(|| native.as_ref().and_then(|native| native.media_ejectable))
            .unwrap_or(removable || network);
        let writable = resource
            .as_ref()
            .and_then(|resource| resource.is_read_only.map(|read_only| !read_only))
            .or_else(|| native.as_ref().and_then(|native| native.media_writable))
            .unwrap_or_else(|| !metadata.permissions().readonly());
        let read_only = resource
            .as_ref()
            .and_then(|resource| resource.is_read_only)
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
            case_sensitive,
            case_preserving,
            local,
            internal,
            mountable,
            capacity,
            commands,
            native_status,
            resource_status,
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
            source: enrich_volume_source(source, resource_status, resource.as_ref()),
        })
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "volume\t{}\t{}\tpath={}\tkind={}\tmount={}\tremovable={}\tnetwork={}\tejectable={}\ttotal={}\tavailable={}\teject={}\tmount={}\tunmount={}\tsource={}\treason={}\tstable-id={}\tnative-status={}\twritable={}\tread-only={}\tcase-sensitive={}\tcase-preserving={}\tlocal={}\tinternal={}\tmountable={}\tbsd={}\tvolume-uuid={}\tmedia-uuid={}\tfs={}\tmedia-content={}\tprotocol={}\tmodel={}\tvendor={}\tresource-status={}",
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
                .unwrap_or("-")
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
    Submitted,
    Refused,
    Unsupported,
    Failed,
}

impl VolumeOperationDisposition {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Submitted => "submitted",
            Self::Refused => "refused",
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
                volume,
                "native-mount-requires-unmounted-disk-identity",
            ));
        }
        if volume.source.starts_with("fixture-marker:") {
            return Ok(Self::with_volume(
                operation,
                VolumeOperationDisposition::Refused,
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
                volume,
                "system-volume-operation-refused",
            ));
        }
        if !path.starts_with("/Volumes") {
            return Ok(Self::with_volume(
                operation,
                VolumeOperationDisposition::Refused,
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
                volume,
                "diskarbitration-volume-unavailable",
            ));
        }
        if let Some(reason) = disabled_command_reason(operation, &volume) {
            return Ok(Self::with_volume(
                operation,
                VolumeOperationDisposition::Refused,
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
        let disposition = match native.status {
            NativeVolumeOperationStatus::Submitted => VolumeOperationDisposition::Submitted,
            NativeVolumeOperationStatus::Missing | NativeVolumeOperationStatus::Unavailable => {
                VolumeOperationDisposition::Failed
            }
        };
        Ok(Self::with_volume(
            operation,
            disposition,
            Some(native.status),
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
            volume: None,
            reason: reason.into(),
        }
    }

    fn with_volume(
        operation: VolumeOperation,
        disposition: VolumeOperationDisposition,
        native_status: Option<NativeVolumeOperationStatus>,
        volume: VolumeDescriptor,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            path: volume.path.clone(),
            operation,
            disposition,
            native_status,
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
            "volume-operation\t{}\tpath={}\tdisposition={}\tnative-status={}\tvolume-kind={}\tmount={}\tstable-id={}\treason={}",
            self.operation.as_str(),
            self.path.display(),
            self.disposition.as_str(),
            self.native_status
                .map(NativeVolumeOperationStatus::as_str)
                .unwrap_or("-"),
            kind,
            mount,
            stable_identity,
            escape_field(&self.reason)
        )
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

fn classify_volume(
    path: &Path,
    marker: Option<&str>,
    native: Option<&gfm_mac_sys::NativeVolumeDescription>,
    resource: Option<&NativeVolumeResourceValues>,
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

    if let Some(kind) = classify_native_volume(path, native, resource) {
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
) -> String {
    let Some(status) = resource_status else {
        return source;
    };
    let mut source = format!("{source};url-resource={}", status.as_str());
    if let Some(reason) = resource.and_then(|resource| resource.reason.as_deref()) {
        source.push(':');
        source.push_str(&escape_field(reason));
    }
    source
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
        assert_eq!(
            descriptor.resource_status,
            Some(NativeVolumeStatus::Available)
        );
        assert_eq!(descriptor.read_only, !descriptor.writable);
        assert!(descriptor.stable_identity.starts_with("diskarbitration:"));
        assert_eq!(descriptor.commands.eject, VolumeCommandState::Hidden);
        assert!(descriptor.capacity.total_bytes > 0);
        assert!(descriptor.as_tsv().contains("\tnative-status=available\t"));
        assert!(descriptor.as_tsv().contains("\tstable-id="));
        assert!(descriptor.as_tsv().contains("\tread-only="));
        assert!(descriptor.as_tsv().contains("\tcase-sensitive="));
        assert!(descriptor.as_tsv().contains("\tlocal="));
        assert!(descriptor.as_tsv().contains("\tresource-status=available"));
        assert!(descriptor.source.contains("url-resource=available"));
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
        assert_eq!(descriptor.resource_status, None);
        assert!(descriptor.stable_identity.starts_with("dev:"));
        assert_eq!(descriptor.commands.eject, VolumeCommandState::Enabled);
        assert!(descriptor
            .as_tsv()
            .contains("source=fixture-marker:external-removable"));
        assert_eq!(descriptor.case_sensitive, None);
        assert!(descriptor.as_tsv().contains("\tresource-status=-"));
        assert!(!descriptor.source.contains("url-resource="));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn classify_native_volume_uses_url_resource_locality() {
        let resource = resource_values(|values| {
            values.is_local = Some(false);
        });

        let kind = classify_native_volume(Path::new("/Volumes/Team Share"), None, Some(&resource));

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
    fn volume_operation_refuses_system_root() {
        let report = VolumeOperationReport::execute("/", VolumeOperation::Unmount).unwrap();

        assert_eq!(report.operation, VolumeOperation::Unmount);
        assert_eq!(report.disposition, VolumeOperationDisposition::Refused);
        assert_eq!(report.native_status, None);
        assert_eq!(report.reason, "system-volume-operation-refused");
        assert!(report.as_tsv().contains("\tdisposition=refused\t"));
    }

    #[test]
    fn volume_operation_refuses_fixture_volume_before_native_call() {
        let root = unique_temp_dir("gfm-volume-operation-fixture");
        fs::write(root.join(VOLUME_MARKER), "external-removable\n").unwrap();

        let report = VolumeOperationReport::execute(&root, VolumeOperation::Eject).unwrap();

        assert_eq!(report.disposition, VolumeOperationDisposition::Refused);
        assert_eq!(report.native_status, None);
        assert_eq!(report.reason, "fixture-volume-native-operation-disabled");
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
            is_ejectable: None,
            is_internal: None,
            is_local: None,
            is_read_only: None,
            is_removable: None,
            supports_case_preserved_names: None,
            supports_case_sensitive_names: None,
            reason: None,
        };
        configure(&mut values);
        values
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
