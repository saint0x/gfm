use core_foundation::base::{kCFAllocatorDefault, CFType, TCFType};
use core_foundation::boolean::{CFBoolean, CFBooleanRef};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_foundation::url::CFURL;
use core_foundation_sys::base::{CFAllocatorRef, CFGetTypeID, CFRelease, CFTypeRef};
use core_foundation_sys::dictionary::{CFDictionaryGetValueIfPresent, CFDictionaryRef};
use core_foundation_sys::string::CFStringRef;
use core_foundation_sys::url::CFURLRef;
use core_foundation_sys::uuid::{CFUUIDCreateString, CFUUIDRef};
use libc::{c_void, statfs, MNT_LOCAL, MNT_NOWAIT, MNT_RDONLY};
use std::ffi::{CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr;

type DASessionRef = *const c_void;
type DADiskRef = *const c_void;
type DADissenterRef = *const c_void;
type DADiskEjectCallback = Option<unsafe extern "C" fn(DADiskRef, DADissenterRef, *mut c_void)>;
type DADiskUnmountCallback = Option<unsafe extern "C" fn(DADiskRef, DADissenterRef, *mut c_void)>;

#[link(name = "DiskArbitration", kind = "framework")]
extern "C" {
    fn DASessionCreate(allocator: CFAllocatorRef) -> DASessionRef;
    fn DADiskCreateFromVolumePath(
        allocator: CFAllocatorRef,
        session: DASessionRef,
        path: CFURLRef,
    ) -> DADiskRef;
    fn DADiskCopyDescription(disk: DADiskRef) -> CFDictionaryRef;
    fn DADiskEject(
        disk: DADiskRef,
        options: u32,
        callback: DADiskEjectCallback,
        context: *mut c_void,
    );
    fn DADiskUnmount(
        disk: DADiskRef,
        options: u32,
        callback: DADiskUnmountCallback,
        context: *mut c_void,
    );

    static kDADiskDescriptionDeviceInternalKey: CFStringRef;
    static kDADiskDescriptionDeviceModelKey: CFStringRef;
    static kDADiskDescriptionDevicePathKey: CFStringRef;
    static kDADiskDescriptionDeviceProtocolKey: CFStringRef;
    static kDADiskDescriptionDeviceVendorKey: CFStringRef;
    static kDADiskDescriptionMediaBlockSizeKey: CFStringRef;
    static kDADiskDescriptionMediaBSDMajorKey: CFStringRef;
    static kDADiskDescriptionMediaBSDMinorKey: CFStringRef;
    static kDADiskDescriptionMediaBSDNameKey: CFStringRef;
    static kDADiskDescriptionMediaBSDUnitKey: CFStringRef;
    static kDADiskDescriptionMediaContentKey: CFStringRef;
    static kDADiskDescriptionMediaEncryptedKey: CFStringRef;
    static kDADiskDescriptionMediaEjectableKey: CFStringRef;
    static kDADiskDescriptionMediaKindKey: CFStringRef;
    static kDADiskDescriptionMediaLeafKey: CFStringRef;
    static kDADiskDescriptionMediaNameKey: CFStringRef;
    static kDADiskDescriptionMediaPathKey: CFStringRef;
    static kDADiskDescriptionMediaRemovableKey: CFStringRef;
    static kDADiskDescriptionMediaSizeKey: CFStringRef;
    static kDADiskDescriptionMediaTypeKey: CFStringRef;
    static kDADiskDescriptionMediaUUIDKey: CFStringRef;
    static kDADiskDescriptionMediaWholeKey: CFStringRef;
    static kDADiskDescriptionMediaWritableKey: CFStringRef;
    static kDADiskDescriptionVolumeKindKey: CFStringRef;
    static kDADiskDescriptionVolumeMountableKey: CFStringRef;
    static kDADiskDescriptionVolumeNameKey: CFStringRef;
    static kDADiskDescriptionVolumeNetworkKey: CFStringRef;
    static kDADiskDescriptionVolumePathKey: CFStringRef;
    static kDADiskDescriptionVolumeTypeKey: CFStringRef;
    static kDADiskDescriptionVolumeUUIDKey: CFStringRef;
}

#[link(name = "Foundation", kind = "framework")]
extern "C" {
    static NSURLVolumeIsAutomountedKey: CFStringRef;
    static NSURLVolumeIsBrowsableKey: CFStringRef;
    static NSURLVolumeIsEjectableKey: CFStringRef;
    static NSURLVolumeIsInternalKey: CFStringRef;
    static NSURLVolumeIsLocalKey: CFStringRef;
    static NSURLVolumeIsReadOnlyKey: CFStringRef;
    static NSURLVolumeIsRemovableKey: CFStringRef;
    static NSURLVolumeURLForRemountingKey: CFStringRef;
    static NSURLVolumeUUIDStringKey: CFStringRef;
    static NSURLVolumeSupportsCasePreservedNamesKey: CFStringRef;
    static NSURLVolumeSupportsCaseSensitiveNamesKey: CFStringRef;

    fn CFURLCopyResourcePropertyForKey(
        url: CFURLRef,
        key: CFStringRef,
        property_value_type_ref_ptr: *mut CFTypeRef,
        error: *mut core_foundation_sys::error::CFErrorRef,
    ) -> core_foundation_sys::base::Boolean;
}

extern "C" {
    fn CFBooleanGetTypeID() -> core_foundation_sys::base::CFTypeID;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeVolumeDescription {
    pub status: NativeVolumeStatus,
    pub volume_name: Option<String>,
    pub volume_kind: Option<String>,
    pub volume_mountable: Option<bool>,
    pub volume_type: Option<String>,
    pub volume_uuid: Option<String>,
    pub volume_path: Option<PathBuf>,
    pub volume_network: Option<bool>,
    pub media_bsd_name: Option<String>,
    pub media_bsd_major: Option<u64>,
    pub media_bsd_minor: Option<u64>,
    pub media_bsd_unit: Option<u64>,
    pub media_content: Option<String>,
    pub media_kind: Option<String>,
    pub media_leaf: Option<bool>,
    pub media_name: Option<String>,
    pub media_path: Option<String>,
    pub media_removable: Option<bool>,
    pub media_ejectable: Option<bool>,
    pub media_writable: Option<bool>,
    pub media_type: Option<String>,
    pub media_uuid: Option<String>,
    pub media_whole: Option<bool>,
    pub media_encrypted: Option<bool>,
    pub media_block_size_bytes: Option<u64>,
    pub media_size_bytes: Option<u64>,
    pub device_internal: Option<bool>,
    pub device_model: Option<String>,
    pub device_path: Option<String>,
    pub device_protocol: Option<String>,
    pub device_vendor: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeVolumeResourceValues {
    pub status: NativeVolumeStatus,
    pub is_automounted: Option<bool>,
    pub is_browsable: Option<bool>,
    pub is_ejectable: Option<bool>,
    pub is_internal: Option<bool>,
    pub is_local: Option<bool>,
    pub is_read_only: Option<bool>,
    pub is_removable: Option<bool>,
    pub remount_url: Option<String>,
    pub supports_case_preserved_names: Option<bool>,
    pub supports_case_sensitive_names: Option<bool>,
    pub volume_uuid: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeVolumeMountTableEntry {
    pub status: NativeVolumeStatus,
    pub mount_point: Option<PathBuf>,
    pub mounted_from: Option<String>,
    pub filesystem_type: Option<String>,
    pub flags: Option<u32>,
    pub is_read_only: Option<bool>,
    pub is_local: Option<bool>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeVolumeMountTable {
    pub status: NativeVolumeStatus,
    pub entries: Vec<NativeVolumeMountTableEntry>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeVolumeStatus {
    Available,
    Missing,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeVolumeOperation {
    Eject,
    Unmount,
}

impl NativeVolumeOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Eject => "eject",
            Self::Unmount => "unmount",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeVolumeOperationStatus {
    Submitted,
    Missing,
    Unavailable,
}

impl NativeVolumeOperationStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Submitted => "submitted",
            Self::Missing => "missing",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeVolumeOperationResult {
    pub operation: NativeVolumeOperation,
    pub status: NativeVolumeOperationStatus,
    pub reason: Option<String>,
}

impl NativeVolumeStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Missing => "missing",
            Self::Unavailable => "unavailable",
        }
    }
}

pub fn copy_volume_description_for_path(path: &Path) -> NativeVolumeDescription {
    if !path.exists() {
        return missing(format!("volume path does not exist: {}", path.display()));
    }
    let Some(url) = CFURL::from_path(path, true) else {
        return unavailable(format!("invalid volume path URL: {}", path.display()));
    };

    let session = unsafe { DASessionCreate(kCFAllocatorDefault) };
    if session.is_null() {
        return unavailable("DiskArbitration did not create a session");
    }

    let disk = unsafe {
        DADiskCreateFromVolumePath(kCFAllocatorDefault, session, url.as_concrete_TypeRef())
    };
    if disk.is_null() {
        unsafe {
            CFRelease(session as CFTypeRef);
        }
        return unavailable(format!(
            "DiskArbitration did not return a disk for {}",
            path.display()
        ));
    }

    let description = unsafe { DADiskCopyDescription(disk) };
    unsafe {
        CFRelease(disk as CFTypeRef);
        CFRelease(session as CFTypeRef);
    }
    if description.is_null() {
        return unavailable(format!(
            "DiskArbitration did not return a description for {}",
            path.display()
        ));
    }

    let description = unsafe {
        CFDictionary::<*const c_void, *const c_void>::wrap_under_create_rule(description)
    };
    NativeVolumeDescription {
        status: NativeVolumeStatus::Available,
        volume_name: string_value(&description, unsafe { kDADiskDescriptionVolumeNameKey }),
        volume_kind: string_value(&description, unsafe { kDADiskDescriptionVolumeKindKey }),
        volume_mountable: bool_value(&description, unsafe {
            kDADiskDescriptionVolumeMountableKey
        }),
        volume_type: string_value(&description, unsafe { kDADiskDescriptionVolumeTypeKey }),
        volume_uuid: uuid_value(&description, unsafe { kDADiskDescriptionVolumeUUIDKey }),
        volume_path: url_value(&description, unsafe { kDADiskDescriptionVolumePathKey }),
        volume_network: bool_value(&description, unsafe { kDADiskDescriptionVolumeNetworkKey }),
        media_bsd_name: string_value(&description, unsafe { kDADiskDescriptionMediaBSDNameKey }),
        media_bsd_major: u64_value(&description, unsafe { kDADiskDescriptionMediaBSDMajorKey }),
        media_bsd_minor: u64_value(&description, unsafe { kDADiskDescriptionMediaBSDMinorKey }),
        media_bsd_unit: u64_value(&description, unsafe { kDADiskDescriptionMediaBSDUnitKey }),
        media_content: string_value(&description, unsafe { kDADiskDescriptionMediaContentKey }),
        media_kind: string_value(&description, unsafe { kDADiskDescriptionMediaKindKey }),
        media_leaf: bool_value(&description, unsafe { kDADiskDescriptionMediaLeafKey }),
        media_name: string_value(&description, unsafe { kDADiskDescriptionMediaNameKey }),
        media_path: string_value(&description, unsafe { kDADiskDescriptionMediaPathKey }),
        media_removable: bool_value(&description, unsafe { kDADiskDescriptionMediaRemovableKey }),
        media_ejectable: bool_value(&description, unsafe { kDADiskDescriptionMediaEjectableKey }),
        media_writable: bool_value(&description, unsafe { kDADiskDescriptionMediaWritableKey }),
        media_type: string_value(&description, unsafe { kDADiskDescriptionMediaTypeKey }),
        media_uuid: uuid_value(&description, unsafe { kDADiskDescriptionMediaUUIDKey }),
        media_whole: bool_value(&description, unsafe { kDADiskDescriptionMediaWholeKey }),
        media_encrypted: bool_value(&description, unsafe { kDADiskDescriptionMediaEncryptedKey }),
        media_block_size_bytes: u64_value(&description, unsafe {
            kDADiskDescriptionMediaBlockSizeKey
        }),
        media_size_bytes: u64_value(&description, unsafe { kDADiskDescriptionMediaSizeKey }),
        device_internal: bool_value(&description, unsafe { kDADiskDescriptionDeviceInternalKey }),
        device_model: string_value(&description, unsafe { kDADiskDescriptionDeviceModelKey }),
        device_path: string_value(&description, unsafe { kDADiskDescriptionDevicePathKey }),
        device_protocol: string_value(&description, unsafe { kDADiskDescriptionDeviceProtocolKey }),
        device_vendor: string_value(&description, unsafe { kDADiskDescriptionDeviceVendorKey }),
        reason: None,
    }
}

pub fn submit_volume_operation(
    path: &Path,
    operation: NativeVolumeOperation,
) -> NativeVolumeOperationResult {
    let Some((session, disk)) = create_disk_for_volume_path(path) else {
        return NativeVolumeOperationResult {
            operation,
            status: if path.exists() {
                NativeVolumeOperationStatus::Unavailable
            } else {
                NativeVolumeOperationStatus::Missing
            },
            reason: Some(if path.exists() {
                format!(
                    "DiskArbitration did not return a disk for {}",
                    path.display()
                )
            } else {
                format!("volume path does not exist: {}", path.display())
            }),
        };
    };

    match operation {
        NativeVolumeOperation::Eject => unsafe {
            DADiskEject(disk, 0, None, ptr::null_mut());
        },
        NativeVolumeOperation::Unmount => unsafe {
            DADiskUnmount(disk, 0, None, ptr::null_mut());
        },
    }

    unsafe {
        CFRelease(disk as CFTypeRef);
        CFRelease(session as CFTypeRef);
    }

    NativeVolumeOperationResult {
        operation,
        status: NativeVolumeOperationStatus::Submitted,
        reason: Some("submitted-to-diskarbitration".to_string()),
    }
}

pub fn copy_volume_resource_values(path: &Path) -> NativeVolumeResourceValues {
    if !path.exists() {
        return unavailable_resource_values(
            NativeVolumeStatus::Missing,
            format!("volume path does not exist: {}", path.display()),
        );
    }
    let Some(url) = CFURL::from_path(path, path.is_dir()) else {
        return unavailable_resource_values(
            NativeVolumeStatus::Unavailable,
            format!("invalid volume path URL: {}", path.display()),
        );
    };
    let url = url.as_concrete_TypeRef();

    NativeVolumeResourceValues {
        status: NativeVolumeStatus::Available,
        is_automounted: copy_resource_bool(url, unsafe { NSURLVolumeIsAutomountedKey }),
        is_browsable: copy_resource_bool(url, unsafe { NSURLVolumeIsBrowsableKey }),
        is_ejectable: copy_resource_bool(url, unsafe { NSURLVolumeIsEjectableKey }),
        is_internal: copy_resource_bool(url, unsafe { NSURLVolumeIsInternalKey }),
        is_local: copy_resource_bool(url, unsafe { NSURLVolumeIsLocalKey }),
        is_read_only: copy_resource_bool(url, unsafe { NSURLVolumeIsReadOnlyKey }),
        is_removable: copy_resource_bool(url, unsafe { NSURLVolumeIsRemovableKey }),
        remount_url: copy_resource_url_string(url, unsafe { NSURLVolumeURLForRemountingKey }),
        supports_case_preserved_names: copy_resource_bool(url, unsafe {
            NSURLVolumeSupportsCasePreservedNamesKey
        }),
        supports_case_sensitive_names: copy_resource_bool(url, unsafe {
            NSURLVolumeSupportsCaseSensitiveNamesKey
        }),
        volume_uuid: copy_resource_string(url, unsafe { NSURLVolumeUUIDStringKey }),
        reason: None,
    }
}

pub fn copy_volume_mount_table_entry(path: &Path) -> NativeVolumeMountTableEntry {
    if !path.exists() {
        return unavailable_mount_table_entry(
            NativeVolumeStatus::Missing,
            format!("volume path does not exist: {}", path.display()),
        );
    }
    let display_path = path.display().to_string();
    let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
        return unavailable_mount_table_entry(
            NativeVolumeStatus::Unavailable,
            format!("volume path contains an interior NUL: {display_path}"),
        );
    };
    let mut info = std::mem::MaybeUninit::<statfs>::uninit();
    let copied = unsafe { libc::statfs(c_path.as_ptr(), info.as_mut_ptr()) };
    if copied != 0 {
        let error = std::io::Error::last_os_error();
        return unavailable_mount_table_entry(
            NativeVolumeStatus::Unavailable,
            format!("statfs failed for {display_path}: {error}"),
        );
    }
    native_mount_table_entry(unsafe { info.assume_init() })
}

pub fn copy_volume_mount_table() -> NativeVolumeMountTable {
    let mut mounts = ptr::null_mut::<statfs>();
    let count = unsafe { libc::getmntinfo(&mut mounts, MNT_NOWAIT) };
    if count <= 0 || mounts.is_null() {
        let error = std::io::Error::last_os_error();
        return NativeVolumeMountTable {
            status: NativeVolumeStatus::Unavailable,
            entries: Vec::new(),
            reason: Some(format!("getmntinfo failed: {error}")),
        };
    }

    let entries = unsafe { std::slice::from_raw_parts(mounts, count as usize) }
        .iter()
        .copied()
        .map(native_mount_table_entry)
        .collect();
    NativeVolumeMountTable {
        status: NativeVolumeStatus::Available,
        entries,
        reason: None,
    }
}

fn create_disk_for_volume_path(path: &Path) -> Option<(DASessionRef, DADiskRef)> {
    if !path.exists() {
        return None;
    }
    let url = CFURL::from_path(path, true)?;
    let session = unsafe { DASessionCreate(kCFAllocatorDefault) };
    if session.is_null() {
        return None;
    }
    let disk = unsafe {
        DADiskCreateFromVolumePath(kCFAllocatorDefault, session, url.as_concrete_TypeRef())
    };
    if disk.is_null() {
        unsafe {
            CFRelease(session as CFTypeRef);
        }
        return None;
    }
    Some((session, disk))
}

fn string_value(
    description: &CFDictionary<*const c_void, *const c_void>,
    key: CFStringRef,
) -> Option<String> {
    value_for_key(description, key)
        .map(|raw| unsafe { CFString::wrap_under_get_rule(raw as CFStringRef) })
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty())
}

fn bool_value(
    description: &CFDictionary<*const c_void, *const c_void>,
    key: CFStringRef,
) -> Option<bool> {
    value_for_key(description, key)
        .map(|raw| unsafe { CFBoolean::wrap_under_get_rule(raw as _) })
        .map(bool::from)
}

fn u64_value(
    description: &CFDictionary<*const c_void, *const c_void>,
    key: CFStringRef,
) -> Option<u64> {
    value_for_key(description, key)
        .and_then(|raw| unsafe { CFNumber::wrap_under_get_rule(raw as _) }.to_i64())
        .and_then(|value| u64::try_from(value).ok())
}

fn uuid_value(
    description: &CFDictionary<*const c_void, *const c_void>,
    key: CFStringRef,
) -> Option<String> {
    value_for_key(description, key)
        .and_then(|raw| {
            let value = unsafe { CFUUIDCreateString(kCFAllocatorDefault, raw as CFUUIDRef) };
            (!value.is_null()).then_some(value)
        })
        .map(|value| unsafe { CFString::wrap_under_create_rule(value) })
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty())
}

fn url_value(
    description: &CFDictionary<*const c_void, *const c_void>,
    key: CFStringRef,
) -> Option<PathBuf> {
    value_for_key(description, key)
        .map(|raw| unsafe { CFURL::wrap_under_get_rule(raw as CFURLRef) })
        .and_then(|url| url.to_path())
}

fn copy_resource_bool(url: CFURLRef, key: CFStringRef) -> Option<bool> {
    let value = copy_resource_value(url, key)?;
    if unsafe { CFGetTypeID(value.as_CFTypeRef()) } != unsafe { CFBooleanGetTypeID() } {
        return None;
    }
    let typed = unsafe { CFBoolean::wrap_under_get_rule(value.as_CFTypeRef() as CFBooleanRef) };
    Some(typed.into())
}

fn copy_resource_string(url: CFURLRef, key: CFStringRef) -> Option<String> {
    copy_resource_value(url, key)?
        .downcast::<CFString>()
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty())
}

fn copy_resource_url_string(url: CFURLRef, key: CFStringRef) -> Option<String> {
    copy_resource_value(url, key)?
        .downcast::<CFURL>()
        .map(|url| url.get_string().to_string())
        .filter(|value| !value.is_empty())
}

fn copy_resource_value(url: CFURLRef, key: CFStringRef) -> Option<CFType> {
    let mut value: CFTypeRef = ptr::null();
    let copied = unsafe { CFURLCopyResourcePropertyForKey(url, key, &mut value, ptr::null_mut()) };
    if copied == 0 || value.is_null() {
        None
    } else {
        Some(unsafe { CFType::wrap_under_create_rule(value) })
    }
}

fn c_char_array_to_string(buffer: &[libc::c_char]) -> Option<String> {
    if buffer.first().copied().unwrap_or_default() == 0 {
        return None;
    }
    let value = unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    (!value.is_empty()).then_some(value)
}

fn native_mount_table_entry(info: statfs) -> NativeVolumeMountTableEntry {
    let flags = info.f_flags;
    NativeVolumeMountTableEntry {
        status: NativeVolumeStatus::Available,
        mount_point: c_char_array_to_string(&info.f_mntonname).map(PathBuf::from),
        mounted_from: c_char_array_to_string(&info.f_mntfromname),
        filesystem_type: c_char_array_to_string(&info.f_fstypename),
        flags: Some(flags),
        is_read_only: Some((flags & MNT_RDONLY as u32) != 0),
        is_local: Some((flags & MNT_LOCAL as u32) != 0),
        reason: None,
    }
}

fn value_for_key(
    description: &CFDictionary<*const c_void, *const c_void>,
    key: CFStringRef,
) -> Option<CFTypeRef> {
    let mut value = ptr::null();
    let present = unsafe {
        CFDictionaryGetValueIfPresent(
            description.as_concrete_TypeRef(),
            key as *const c_void,
            &mut value,
        )
    };
    (present != 0 && !value.is_null()).then_some(value as CFTypeRef)
}

fn missing(reason: impl Into<String>) -> NativeVolumeDescription {
    unavailable_with_status(NativeVolumeStatus::Missing, reason)
}

fn unavailable(reason: impl Into<String>) -> NativeVolumeDescription {
    unavailable_with_status(NativeVolumeStatus::Unavailable, reason)
}

fn unavailable_with_status(
    status: NativeVolumeStatus,
    reason: impl Into<String>,
) -> NativeVolumeDescription {
    NativeVolumeDescription {
        status,
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
        reason: Some(reason.into()),
    }
}

fn unavailable_resource_values(
    status: NativeVolumeStatus,
    reason: impl Into<String>,
) -> NativeVolumeResourceValues {
    NativeVolumeResourceValues {
        status,
        is_automounted: None,
        is_browsable: None,
        is_ejectable: None,
        is_internal: None,
        is_local: None,
        is_read_only: None,
        is_removable: None,
        remount_url: None,
        supports_case_preserved_names: None,
        supports_case_sensitive_names: None,
        volume_uuid: None,
        reason: Some(reason.into()),
    }
}

fn unavailable_mount_table_entry(
    status: NativeVolumeStatus,
    reason: impl Into<String>,
) -> NativeVolumeMountTableEntry {
    NativeVolumeMountTableEntry {
        status,
        mount_point: None,
        mounted_from: None,
        filesystem_type: None,
        flags: None,
        is_read_only: None,
        is_local: None,
        reason: Some(reason.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_root_volume_description() {
        let description = copy_volume_description_for_path(Path::new("/"));

        assert_eq!(description.status, NativeVolumeStatus::Available);
        assert!(description.volume_path.is_some());
        assert!(
            description.volume_name.is_some()
                || description.volume_kind.is_some()
                || description.media_bsd_name.is_some()
        );
    }

    #[test]
    fn reports_missing_paths_without_diskarbitration_call() {
        let description = copy_volume_description_for_path(Path::new(
            "/tmp/gfm-native-volume-description-missing",
        ));

        assert_eq!(description.status, NativeVolumeStatus::Missing);
        assert!(description.reason.unwrap().contains("does not exist"));
    }

    #[test]
    fn resolves_root_volume_resource_values() {
        let values = copy_volume_resource_values(Path::new("/"));

        assert_eq!(values.status, NativeVolumeStatus::Available);
        assert!(values.is_local.is_some() || values.is_read_only.is_some());
        assert!(values.is_browsable.is_some() || values.volume_uuid.is_some());
    }

    #[test]
    fn resolves_root_mount_table_entry() {
        let entry = copy_volume_mount_table_entry(Path::new("/"));

        assert_eq!(entry.status, NativeVolumeStatus::Available);
        assert!(entry.mount_point.is_some());
        assert!(entry.filesystem_type.is_some());
        assert!(entry.flags.is_some());
    }

    #[test]
    fn resolves_current_mount_table_snapshot() {
        let table = copy_volume_mount_table();

        assert_eq!(table.status, NativeVolumeStatus::Available);
        assert!(table.entries.iter().any(|entry| {
            entry.mount_point.as_deref() == Some(Path::new("/"))
                && entry.filesystem_type.is_some()
                && entry.flags.is_some()
        }));
    }

    #[test]
    fn missing_volume_operation_does_not_submit_to_diskarbitration() {
        let result = submit_volume_operation(
            Path::new("/tmp/gfm-native-volume-operation-missing"),
            NativeVolumeOperation::Eject,
        );

        assert_eq!(result.operation, NativeVolumeOperation::Eject);
        assert_eq!(result.status, NativeVolumeOperationStatus::Missing);
        assert!(result.reason.unwrap().contains("does not exist"));
    }
}
