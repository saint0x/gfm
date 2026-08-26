use core_foundation::base::{kCFAllocatorDefault, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_foundation::url::CFURL;
use core_foundation_sys::base::{CFAllocatorRef, CFRelease, CFTypeRef};
use core_foundation_sys::dictionary::{CFDictionaryGetValueIfPresent, CFDictionaryRef};
use core_foundation_sys::string::CFStringRef;
use core_foundation_sys::url::CFURLRef;
use libc::c_void;
use std::path::{Path, PathBuf};
use std::ptr;

type DASessionRef = *const c_void;
type DADiskRef = *const c_void;

#[link(name = "DiskArbitration", kind = "framework")]
extern "C" {
    fn DASessionCreate(allocator: CFAllocatorRef) -> DASessionRef;
    fn DADiskCreateFromVolumePath(
        allocator: CFAllocatorRef,
        session: DASessionRef,
        path: CFURLRef,
    ) -> DADiskRef;
    fn DADiskCopyDescription(disk: DADiskRef) -> CFDictionaryRef;

    static kDADiskDescriptionDeviceInternalKey: CFStringRef;
    static kDADiskDescriptionDeviceModelKey: CFStringRef;
    static kDADiskDescriptionDeviceProtocolKey: CFStringRef;
    static kDADiskDescriptionDeviceVendorKey: CFStringRef;
    static kDADiskDescriptionMediaBSDNameKey: CFStringRef;
    static kDADiskDescriptionMediaEjectableKey: CFStringRef;
    static kDADiskDescriptionMediaKindKey: CFStringRef;
    static kDADiskDescriptionMediaRemovableKey: CFStringRef;
    static kDADiskDescriptionMediaSizeKey: CFStringRef;
    static kDADiskDescriptionMediaWritableKey: CFStringRef;
    static kDADiskDescriptionVolumeKindKey: CFStringRef;
    static kDADiskDescriptionVolumeNameKey: CFStringRef;
    static kDADiskDescriptionVolumeNetworkKey: CFStringRef;
    static kDADiskDescriptionVolumePathKey: CFStringRef;
    static kDADiskDescriptionVolumeTypeKey: CFStringRef;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeVolumeDescription {
    pub status: NativeVolumeStatus,
    pub volume_name: Option<String>,
    pub volume_kind: Option<String>,
    pub volume_type: Option<String>,
    pub volume_path: Option<PathBuf>,
    pub volume_network: Option<bool>,
    pub media_bsd_name: Option<String>,
    pub media_kind: Option<String>,
    pub media_removable: Option<bool>,
    pub media_ejectable: Option<bool>,
    pub media_writable: Option<bool>,
    pub media_size_bytes: Option<u64>,
    pub device_internal: Option<bool>,
    pub device_model: Option<String>,
    pub device_protocol: Option<String>,
    pub device_vendor: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeVolumeStatus {
    Available,
    Missing,
    Unavailable,
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
        volume_type: string_value(&description, unsafe { kDADiskDescriptionVolumeTypeKey }),
        volume_path: url_value(&description, unsafe { kDADiskDescriptionVolumePathKey }),
        volume_network: bool_value(&description, unsafe { kDADiskDescriptionVolumeNetworkKey }),
        media_bsd_name: string_value(&description, unsafe { kDADiskDescriptionMediaBSDNameKey }),
        media_kind: string_value(&description, unsafe { kDADiskDescriptionMediaKindKey }),
        media_removable: bool_value(&description, unsafe { kDADiskDescriptionMediaRemovableKey }),
        media_ejectable: bool_value(&description, unsafe { kDADiskDescriptionMediaEjectableKey }),
        media_writable: bool_value(&description, unsafe { kDADiskDescriptionMediaWritableKey }),
        media_size_bytes: u64_value(&description, unsafe { kDADiskDescriptionMediaSizeKey }),
        device_internal: bool_value(&description, unsafe { kDADiskDescriptionDeviceInternalKey }),
        device_model: string_value(&description, unsafe { kDADiskDescriptionDeviceModelKey }),
        device_protocol: string_value(&description, unsafe { kDADiskDescriptionDeviceProtocolKey }),
        device_vendor: string_value(&description, unsafe { kDADiskDescriptionDeviceVendorKey }),
        reason: None,
    }
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

fn url_value(
    description: &CFDictionary<*const c_void, *const c_void>,
    key: CFStringRef,
) -> Option<PathBuf> {
    value_for_key(description, key)
        .map(|raw| unsafe { CFURL::wrap_under_get_rule(raw as CFURLRef) })
        .and_then(|url| url.to_path())
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
        volume_type: None,
        volume_path: None,
        volume_network: None,
        media_bsd_name: None,
        media_kind: None,
        media_removable: None,
        media_ejectable: None,
        media_writable: None,
        media_size_bytes: None,
        device_internal: None,
        device_model: None,
        device_protocol: None,
        device_vendor: None,
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
}
