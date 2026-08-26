use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::{CFBoolean, CFBooleanRef};
use core_foundation::string::{CFString, CFStringRef};
use core_foundation::url::CFURL;
use core_foundation_sys::base::{Boolean, CFGetTypeID, CFTypeRef};
use core_foundation_sys::error::CFErrorRef;
use core_foundation_sys::url::CFURLRef;
use std::path::Path;
use std::ptr;

#[link(name = "Foundation", kind = "framework")]
extern "C" {
    static NSURLIsUbiquitousItemKey: CFStringRef;
    static NSURLUbiquitousItemHasUnresolvedConflictsKey: CFStringRef;
    static NSURLUbiquitousItemIsDownloadingKey: CFStringRef;
    static NSURLUbiquitousItemIsUploadingKey: CFStringRef;
    static NSURLUbiquitousItemIsUploadedKey: CFStringRef;
    static NSURLUbiquitousItemDownloadingStatusKey: CFStringRef;
    static NSURLUbiquitousItemDownloadingStatusNotDownloaded: CFStringRef;
    static NSURLUbiquitousItemDownloadingStatusDownloaded: CFStringRef;
    static NSURLUbiquitousItemDownloadingStatusCurrent: CFStringRef;

    fn CFURLCopyResourcePropertyForKey(
        url: CFURLRef,
        key: CFStringRef,
        property_value_type_ref_ptr: *mut CFTypeRef,
        error: *mut CFErrorRef,
    ) -> Boolean;
}

extern "C" {
    fn CFBooleanGetTypeID() -> core_foundation_sys::base::CFTypeID;
    fn CFStringGetTypeID() -> core_foundation_sys::base::CFTypeID;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeFileProviderResourceValues {
    pub is_ubiquitous: Option<bool>,
    pub has_unresolved_conflicts: Option<bool>,
    pub is_downloading: Option<bool>,
    pub is_uploading: Option<bool>,
    pub is_uploaded: Option<bool>,
    pub downloading_status: Option<NativeUbiquitousDownloadingStatus>,
    pub status: NativeFileProviderStatus,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeUbiquitousDownloadingStatus {
    NotDownloaded,
    Downloaded,
    Current,
    Other,
}

impl NativeUbiquitousDownloadingStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotDownloaded => "not-downloaded",
            Self::Downloaded => "downloaded",
            Self::Current => "current",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeFileProviderStatus {
    Available,
    UnsupportedPath,
}

pub fn copy_fileprovider_resource_values(path: &Path) -> NativeFileProviderResourceValues {
    let Some(url) = CFURL::from_path(path, path.is_dir()) else {
        return unsupported(format!("invalid path URL: {}", path.display()));
    };

    let is_ubiquitous = copy_bool(url.as_concrete_TypeRef(), unsafe {
        NSURLIsUbiquitousItemKey
    });
    let has_unresolved_conflicts = copy_bool(url.as_concrete_TypeRef(), unsafe {
        NSURLUbiquitousItemHasUnresolvedConflictsKey
    });
    let is_downloading = copy_bool(url.as_concrete_TypeRef(), unsafe {
        NSURLUbiquitousItemIsDownloadingKey
    });
    let is_uploading = copy_bool(url.as_concrete_TypeRef(), unsafe {
        NSURLUbiquitousItemIsUploadingKey
    });
    let is_uploaded = copy_bool(url.as_concrete_TypeRef(), unsafe {
        NSURLUbiquitousItemIsUploadedKey
    });
    let downloading_status = copy_downloading_status(url.as_concrete_TypeRef());

    NativeFileProviderResourceValues {
        is_ubiquitous,
        has_unresolved_conflicts,
        is_downloading,
        is_uploading,
        is_uploaded,
        downloading_status,
        status: NativeFileProviderStatus::Available,
        reason: None,
    }
}

fn copy_bool(url: CFURLRef, key: CFStringRef) -> Option<bool> {
    let value = copy_resource_value(url, key)?;
    if unsafe { CFGetTypeID(value.as_CFTypeRef()) } != unsafe { CFBooleanGetTypeID() } {
        return None;
    }
    let typed = unsafe { CFBoolean::wrap_under_get_rule(value.as_CFTypeRef() as CFBooleanRef) };
    Some(typed.into())
}

fn copy_downloading_status(url: CFURLRef) -> Option<NativeUbiquitousDownloadingStatus> {
    let value = copy_resource_value(url, unsafe { NSURLUbiquitousItemDownloadingStatusKey })?;
    if unsafe { CFGetTypeID(value.as_CFTypeRef()) } != unsafe { CFStringGetTypeID() } {
        return None;
    }
    let status = unsafe { CFString::wrap_under_get_rule(value.as_CFTypeRef() as CFStringRef) };

    let not_downloaded =
        unsafe { CFString::wrap_under_get_rule(NSURLUbiquitousItemDownloadingStatusNotDownloaded) };
    let downloaded =
        unsafe { CFString::wrap_under_get_rule(NSURLUbiquitousItemDownloadingStatusDownloaded) };
    let current =
        unsafe { CFString::wrap_under_get_rule(NSURLUbiquitousItemDownloadingStatusCurrent) };

    if status == not_downloaded {
        Some(NativeUbiquitousDownloadingStatus::NotDownloaded)
    } else if status == downloaded {
        Some(NativeUbiquitousDownloadingStatus::Downloaded)
    } else if status == current {
        Some(NativeUbiquitousDownloadingStatus::Current)
    } else {
        Some(NativeUbiquitousDownloadingStatus::Other)
    }
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

fn unsupported(reason: String) -> NativeFileProviderResourceValues {
    NativeFileProviderResourceValues {
        is_ubiquitous: None,
        has_unresolved_conflicts: None,
        is_downloading: None,
        is_uploading: None,
        is_uploaded: None,
        downloading_status: None,
        status: NativeFileProviderStatus::UnsupportedPath,
        reason: Some(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn reads_native_fileprovider_resource_values_for_local_file() {
        let path = std::env::temp_dir().join(format!(
            "gfm-native-fileprovider-{}.txt",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, "local").unwrap();

        let values = copy_fileprovider_resource_values(&path);

        assert_eq!(values.status, NativeFileProviderStatus::Available);
        assert_ne!(values.is_ubiquitous, Some(true));
        fs::remove_file(path).unwrap();
    }
}
