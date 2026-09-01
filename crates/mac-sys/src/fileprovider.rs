use crate::url::{existing_path_url, NativePathUrl};
use block::ConcreteBlock;
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::{CFBoolean, CFBooleanRef};
use core_foundation::string::{CFString, CFStringRef};
use core_foundation_sys::base::{Boolean, CFGetTypeID, CFTypeRef};
use core_foundation_sys::error::CFErrorRef;
use core_foundation_sys::number::{
    kCFNumberDoubleType, kCFNumberSInt64Type, CFNumberGetTypeID, CFNumberGetValue, CFNumberRef,
};
use core_foundation_sys::url::CFURLRef;
use libc::{c_char, c_void};
use objc::runtime::{Class, Object, Sel};
use std::path::Path;
use std::ptr;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[link(name = "Foundation", kind = "framework")]
extern "C" {
    static NSURLIsUbiquitousItemKey: CFStringRef;
    static NSURLUbiquitousItemHasUnresolvedConflictsKey: CFStringRef;
    static NSURLUbiquitousItemIsDownloadedKey: CFStringRef;
    static NSURLUbiquitousItemIsDownloadingKey: CFStringRef;
    static NSURLUbiquitousItemIsUploadingKey: CFStringRef;
    static NSURLUbiquitousItemIsUploadedKey: CFStringRef;
    static NSURLUbiquitousItemDownloadRequestedKey: CFStringRef;
    static NSURLUbiquitousItemPercentDownloadedKey: CFStringRef;
    static NSURLUbiquitousItemPercentUploadedKey: CFStringRef;
    static NSURLUbiquitousItemDownloadingStatusKey: CFStringRef;
    static NSURLUbiquitousItemDownloadingErrorKey: CFStringRef;
    static NSURLUbiquitousItemUploadingErrorKey: CFStringRef;
    static NSURLUbiquitousItemIsExcludedFromSyncKey: CFStringRef;
    static NSURLFileSizeKey: CFStringRef;
    static NSURLFileAllocatedSizeKey: CFStringRef;
    static NSURLTotalFileAllocatedSizeKey: CFStringRef;
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

#[link(name = "FileProvider", kind = "framework")]
extern "C" {}

extern "C" {
    fn CFBooleanGetTypeID() -> core_foundation_sys::base::CFTypeID;
    fn CFStringGetTypeID() -> core_foundation_sys::base::CFTypeID;
}

#[link(name = "objc")]
extern "C" {
    fn objc_msgSend();
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeFileProviderResourceValues {
    pub is_ubiquitous: Option<bool>,
    pub has_unresolved_conflicts: Option<bool>,
    pub is_downloaded: Option<bool>,
    pub is_downloading: Option<bool>,
    pub is_uploading: Option<bool>,
    pub is_uploaded: Option<bool>,
    pub download_requested: Option<bool>,
    pub percent_downloaded_milli: Option<u32>,
    pub percent_uploaded_milli: Option<u32>,
    pub downloading_status: Option<NativeUbiquitousDownloadingStatus>,
    pub downloading_error: Option<NativeUbiquitousError>,
    pub uploading_error: Option<NativeUbiquitousError>,
    pub is_excluded_from_sync: Option<bool>,
    pub file_size_bytes: Option<u64>,
    pub file_allocated_size_bytes: Option<u64>,
    pub total_file_allocated_size_bytes: Option<u64>,
    pub status: NativeFileProviderStatus,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeUbiquitousError {
    pub code: Option<i64>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeFileProviderDomainEnumeration {
    pub status: NativeFileProviderDomainStatus,
    pub domains: Vec<NativeFileProviderDomain>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeFileProviderDomain {
    pub identifier: Option<String>,
    pub display_name: Option<String>,
    pub path_relative_to_document_storage: Option<String>,
    pub disconnected: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeFileProviderDomainStatus {
    Available,
    NoDomains,
    Unsupported,
    PermissionDenied,
    Unavailable,
}

impl NativeFileProviderDomainStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::NoDomains => "no-domains",
            Self::Unsupported => "unsupported",
            Self::PermissionDenied => "permission-denied",
            Self::Unavailable => "unavailable",
        }
    }
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
    Missing,
    UnsupportedPath,
    Unavailable,
}

impl NativeFileProviderStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Missing => "missing",
            Self::UnsupportedPath => "unsupported-path",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeFileProviderIdentity {
    pub status: NativeFileProviderIdentityStatus,
    pub item_identifier: Option<String>,
    pub domain_identifier: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeFileProviderIdentityStatus {
    NotQueried,
    Available,
    NoProviderForPath,
    Missing,
    UnsupportedPath,
    Unavailable,
    ProviderUnavailable,
    TimedOut,
    Failed,
}

impl NativeFileProviderIdentityStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotQueried => "not-queried",
            Self::Available => "available",
            Self::NoProviderForPath => "no-provider-for-path",
            Self::Missing => "missing",
            Self::UnsupportedPath => "unsupported-path",
            Self::Unavailable => "unavailable",
            Self::ProviderUnavailable => "provider-unavailable",
            Self::TimedOut => "timed-out",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeFileProviderOperationResult {
    pub status: NativeFileProviderOperationStatus,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeFileProviderOperationStatus {
    Completed,
    Missing,
    PermissionDenied,
    Unavailable,
    Cancelled,
    Failed,
    UnsupportedPath,
}

pub fn enumerate_fileprovider_domains() -> NativeFileProviderDomainEnumeration {
    let Some(manager_class) = Class::get("NSFileProviderManager") else {
        return domain_enumeration(
            NativeFileProviderDomainStatus::Unsupported,
            Vec::new(),
            "NSFileProviderManager class is unavailable",
        );
    };
    let selector = Sel::register("getDomainsWithCompletionHandler:");
    if unsafe { !class_responds_to_selector(manager_class, selector) } {
        return domain_enumeration(
            NativeFileProviderDomainStatus::Unsupported,
            Vec::new(),
            "NSFileProviderManager getDomainsWithCompletionHandler: is unavailable",
        );
    }

    let state = Arc::new((
        Mutex::new(None::<NativeFileProviderDomainEnumeration>),
        Condvar::new(),
    ));
    let callback_state = Arc::clone(&state);
    let block = ConcreteBlock::new(move |domains: *mut Object, error: *mut Object| {
        let result = if !error.is_null() {
            let reason = unsafe { ns_error_description(error) };
            let status = if reason
                .as_deref()
                .is_some_and(|value| value.contains("permission") || value.contains("denied"))
            {
                NativeFileProviderDomainStatus::PermissionDenied
            } else {
                NativeFileProviderDomainStatus::Unavailable
            };
            NativeFileProviderDomainEnumeration {
                status,
                domains: Vec::new(),
                reason,
            }
        } else {
            let domains = domains_from_nsarray(domains);
            NativeFileProviderDomainEnumeration {
                status: if domains.is_empty() {
                    NativeFileProviderDomainStatus::NoDomains
                } else {
                    NativeFileProviderDomainStatus::Available
                },
                domains,
                reason: None,
            }
        };

        let (lock, completed) = &*callback_state;
        if let Ok(mut slot) = lock.lock() {
            *slot = Some(result);
            completed.notify_one();
        }
    })
    .copy();

    unsafe {
        get_domains_with_completion_handler(manager_class, &*block as *const _ as *mut Object);
    }

    let (lock, completed) = &*state;
    let Ok(slot) = lock.lock() else {
        return domain_enumeration(
            NativeFileProviderDomainStatus::Unavailable,
            Vec::new(),
            "fileprovider domain callback state lock failed",
        );
    };
    let Ok((mut slot, wait)) =
        completed.wait_timeout_while(slot, Duration::from_secs(2), |slot| slot.is_none())
    else {
        return domain_enumeration(
            NativeFileProviderDomainStatus::Unavailable,
            Vec::new(),
            "fileprovider domain callback wait failed",
        );
    };
    if wait.timed_out() && slot.is_none() {
        domain_enumeration(
            NativeFileProviderDomainStatus::Unavailable,
            Vec::new(),
            "NSFileProviderManager domain enumeration timed out",
        )
    } else {
        slot.take().unwrap_or_else(|| {
            domain_enumeration(
                NativeFileProviderDomainStatus::Unavailable,
                Vec::new(),
                "fileprovider domain callback completed without a result",
            )
        })
    }
}

fn domains_from_nsarray(domains: *mut Object) -> Vec<NativeFileProviderDomain> {
    if domains.is_null() {
        return Vec::new();
    }
    let count = nsarray_count(domains);
    (0..count)
        .filter_map(|index| nsarray_object_at_index(domains, index))
        .map(native_domain_from_object)
        .collect()
}

fn native_domain_from_object(domain: *mut Object) -> NativeFileProviderDomain {
    NativeFileProviderDomain {
        identifier: string_property(domain, "identifier"),
        display_name: string_property(domain, "displayName"),
        path_relative_to_document_storage: string_property(domain, "pathRelativeToDocumentStorage"),
        disconnected: bool_property_if_supported(domain, "isDisconnected"),
    }
}

fn nsarray_count(array: *mut Object) -> usize {
    let send: unsafe extern "C" fn(*mut Object, Sel) -> usize =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    unsafe { send(array, Sel::register("count")) }
}

fn nsarray_object_at_index(array: *mut Object, index: usize) -> Option<*mut Object> {
    let send: unsafe extern "C" fn(*mut Object, Sel, usize) -> *mut Object =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    let object = unsafe { send(array, Sel::register("objectAtIndex:"), index) };
    (!object.is_null()).then_some(object)
}

fn string_property(object: *mut Object, selector: &str) -> Option<String> {
    if object.is_null() {
        return None;
    }
    let selector = Sel::register(selector);
    if !object_responds_to_selector(object, selector) {
        return None;
    }
    let send: unsafe extern "C" fn(*mut Object, Sel) -> *mut Object =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    unsafe { ns_string_to_string(send(object, selector)) }
}

fn bool_property_if_supported(object: *mut Object, selector: &str) -> Option<bool> {
    if object.is_null() {
        return None;
    }
    let selector = Sel::register(selector);
    if !object_responds_to_selector(object, selector) {
        return None;
    }
    let send: unsafe extern "C" fn(*mut Object, Sel) -> i8 =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    Some(unsafe { send(object, selector) != 0 })
}

fn object_responds_to_selector(object: *mut Object, selector: Sel) -> bool {
    let responds: unsafe extern "C" fn(*mut Object, Sel, Sel) -> i8 =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    unsafe { responds(object, Sel::register("respondsToSelector:"), selector) != 0 }
}

pub fn copy_fileprovider_resource_values(path: &Path) -> NativeFileProviderResourceValues {
    let url = match existing_path_url(path, "fileprovider path") {
        NativePathUrl::Ready(url) => url,
        NativePathUrl::Missing(reason) => return missing_values(reason),
        NativePathUrl::Unavailable(reason) => return unavailable_values(reason),
        NativePathUrl::Invalid(reason) => return unsupported(reason),
    };

    let mut errors = Vec::new();
    let is_ubiquitous = copy_bool(
        url.as_concrete_TypeRef(),
        unsafe { NSURLIsUbiquitousItemKey },
        "NSURLIsUbiquitousItemKey",
        &mut errors,
    );
    let has_unresolved_conflicts = copy_bool(
        url.as_concrete_TypeRef(),
        unsafe { NSURLUbiquitousItemHasUnresolvedConflictsKey },
        "NSURLUbiquitousItemHasUnresolvedConflictsKey",
        &mut errors,
    );
    let is_downloaded = copy_bool(
        url.as_concrete_TypeRef(),
        unsafe { NSURLUbiquitousItemIsDownloadedKey },
        "NSURLUbiquitousItemIsDownloadedKey",
        &mut errors,
    );
    let is_downloading = copy_bool(
        url.as_concrete_TypeRef(),
        unsafe { NSURLUbiquitousItemIsDownloadingKey },
        "NSURLUbiquitousItemIsDownloadingKey",
        &mut errors,
    );
    let is_uploading = copy_bool(
        url.as_concrete_TypeRef(),
        unsafe { NSURLUbiquitousItemIsUploadingKey },
        "NSURLUbiquitousItemIsUploadingKey",
        &mut errors,
    );
    let is_uploaded = copy_bool(
        url.as_concrete_TypeRef(),
        unsafe { NSURLUbiquitousItemIsUploadedKey },
        "NSURLUbiquitousItemIsUploadedKey",
        &mut errors,
    );
    let download_requested = copy_bool(
        url.as_concrete_TypeRef(),
        unsafe { NSURLUbiquitousItemDownloadRequestedKey },
        "NSURLUbiquitousItemDownloadRequestedKey",
        &mut errors,
    );
    let percent_downloaded_milli = copy_percent_milli(
        url.as_concrete_TypeRef(),
        unsafe { NSURLUbiquitousItemPercentDownloadedKey },
        "NSURLUbiquitousItemPercentDownloadedKey",
        &mut errors,
    );
    let percent_uploaded_milli = copy_percent_milli(
        url.as_concrete_TypeRef(),
        unsafe { NSURLUbiquitousItemPercentUploadedKey },
        "NSURLUbiquitousItemPercentUploadedKey",
        &mut errors,
    );
    let downloading_status = copy_downloading_status(url.as_concrete_TypeRef(), &mut errors);
    let downloading_error = copy_error(
        url.as_concrete_TypeRef(),
        unsafe { NSURLUbiquitousItemDownloadingErrorKey },
        "NSURLUbiquitousItemDownloadingErrorKey",
        &mut errors,
    );
    let uploading_error = copy_error(
        url.as_concrete_TypeRef(),
        unsafe { NSURLUbiquitousItemUploadingErrorKey },
        "NSURLUbiquitousItemUploadingErrorKey",
        &mut errors,
    );
    let is_excluded_from_sync = copy_bool(
        url.as_concrete_TypeRef(),
        unsafe { NSURLUbiquitousItemIsExcludedFromSyncKey },
        "NSURLUbiquitousItemIsExcludedFromSyncKey",
        &mut errors,
    );
    let file_size_bytes = copy_unsigned_size(
        url.as_concrete_TypeRef(),
        unsafe { NSURLFileSizeKey },
        "NSURLFileSizeKey",
        &mut errors,
    );
    let file_allocated_size_bytes = copy_unsigned_size(
        url.as_concrete_TypeRef(),
        unsafe { NSURLFileAllocatedSizeKey },
        "NSURLFileAllocatedSizeKey",
        &mut errors,
    );
    let total_file_allocated_size_bytes = copy_unsigned_size(
        url.as_concrete_TypeRef(),
        unsafe { NSURLTotalFileAllocatedSizeKey },
        "NSURLTotalFileAllocatedSizeKey",
        &mut errors,
    );
    let has_values = is_ubiquitous.is_some()
        || has_unresolved_conflicts.is_some()
        || is_downloaded.is_some()
        || is_downloading.is_some()
        || is_uploading.is_some()
        || is_uploaded.is_some()
        || download_requested.is_some()
        || percent_downloaded_milli.is_some()
        || percent_uploaded_milli.is_some()
        || downloading_status.is_some()
        || downloading_error.is_some()
        || uploading_error.is_some()
        || is_excluded_from_sync.is_some()
        || file_size_bytes.is_some()
        || file_allocated_size_bytes.is_some()
        || total_file_allocated_size_bytes.is_some();
    let status = resource_status_for_values(has_values, &errors);
    let reason = (status == NativeFileProviderStatus::Unavailable)
        .then(|| unavailable_resource_values_reason(path, &errors));

    NativeFileProviderResourceValues {
        is_ubiquitous,
        has_unresolved_conflicts,
        is_downloaded,
        is_downloading,
        is_uploading,
        is_uploaded,
        download_requested,
        percent_downloaded_milli,
        percent_uploaded_milli,
        downloading_status,
        downloading_error,
        uploading_error,
        is_excluded_from_sync,
        file_size_bytes,
        file_allocated_size_bytes,
        total_file_allocated_size_bytes,
        status,
        reason,
    }
}

pub fn copy_fileprovider_identity(path: &Path) -> NativeFileProviderIdentity {
    let url = match existing_path_url(path, "fileprovider identity path") {
        NativePathUrl::Ready(url) => url,
        NativePathUrl::Missing(reason) => {
            return identity_result(NativeFileProviderIdentityStatus::Missing, reason);
        }
        NativePathUrl::Unavailable(reason) => {
            return identity_result(NativeFileProviderIdentityStatus::Unavailable, reason);
        }
        NativePathUrl::Invalid(reason) => {
            return identity_result(NativeFileProviderIdentityStatus::UnsupportedPath, reason);
        }
    };
    let Some(manager_class) = Class::get("NSFileProviderManager") else {
        return identity_result(
            NativeFileProviderIdentityStatus::ProviderUnavailable,
            "NSFileProviderManager class is unavailable",
        );
    };
    let selector = Sel::register("getIdentifierForUserVisibleFileAtURL:completionHandler:");
    if unsafe { !class_responds_to_selector(manager_class, selector) } {
        return identity_result(
            NativeFileProviderIdentityStatus::ProviderUnavailable,
            "NSFileProviderManager getIdentifierForUserVisibleFileAtURL is unavailable",
        );
    }

    let state = Arc::new((
        Mutex::new(None::<NativeFileProviderIdentity>),
        Condvar::new(),
    ));
    let callback_state = Arc::clone(&state);
    let block = ConcreteBlock::new(
        move |item_identifier: *mut Object, domain_identifier: *mut Object, error: *mut Object| {
            let report = if !item_identifier.is_null() && !domain_identifier.is_null() {
                NativeFileProviderIdentity {
                    status: NativeFileProviderIdentityStatus::Available,
                    item_identifier: unsafe { ns_string_to_string(item_identifier) },
                    domain_identifier: unsafe { ns_string_to_string(domain_identifier) },
                    reason: None,
                }
            } else if !error.is_null() {
                NativeFileProviderIdentity {
                    status: NativeFileProviderIdentityStatus::NoProviderForPath,
                    item_identifier: None,
                    domain_identifier: None,
                    reason: unsafe { ns_error_description(error) },
                }
            } else {
                NativeFileProviderIdentity {
                    status: NativeFileProviderIdentityStatus::NoProviderForPath,
                    item_identifier: None,
                    domain_identifier: None,
                    reason: Some("NSFileProviderManager returned no identity".to_string()),
                }
            };
            let (lock, completed) = &*callback_state;
            if let Ok(mut slot) = lock.lock() {
                *slot = Some(report);
                completed.notify_one();
            }
        },
    )
    .copy();

    unsafe {
        get_identifier_for_user_visible_file(
            manager_class,
            url.as_concrete_TypeRef() as *mut Object,
            &*block as *const _ as *mut Object,
        );
    }

    let (lock, completed) = &*state;
    let Ok(slot) = lock.lock() else {
        return identity_result(
            NativeFileProviderIdentityStatus::Failed,
            "fileprovider identity callback state lock failed",
        );
    };
    let Ok((mut slot, wait)) =
        completed.wait_timeout_while(slot, Duration::from_secs(2), |slot| slot.is_none())
    else {
        return identity_result(
            NativeFileProviderIdentityStatus::Failed,
            "fileprovider identity callback wait failed",
        );
    };
    if wait.timed_out() && slot.is_none() {
        identity_result(
            NativeFileProviderIdentityStatus::TimedOut,
            format!(
                "NSFileProviderManager identity lookup timed out for {}",
                path.display()
            ),
        )
    } else {
        slot.take().unwrap_or_else(|| {
            identity_result(
                NativeFileProviderIdentityStatus::Failed,
                "fileprovider identity callback completed without a result",
            )
        })
    }
}

fn copy_bool(
    url: CFURLRef,
    key: CFStringRef,
    key_name: &'static str,
    errors: &mut Vec<String>,
) -> Option<bool> {
    let value = copy_resource_value(url, key, key_name, errors)?;
    if unsafe { CFGetTypeID(value.as_CFTypeRef()) } != unsafe { CFBooleanGetTypeID() } {
        return None;
    }
    let typed = unsafe { CFBoolean::wrap_under_get_rule(value.as_CFTypeRef() as CFBooleanRef) };
    Some(typed.into())
}

fn copy_downloading_status(
    url: CFURLRef,
    errors: &mut Vec<String>,
) -> Option<NativeUbiquitousDownloadingStatus> {
    let value = copy_resource_value(
        url,
        unsafe { NSURLUbiquitousItemDownloadingStatusKey },
        "NSURLUbiquitousItemDownloadingStatusKey",
        errors,
    )?;
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

fn copy_percent_milli(
    url: CFURLRef,
    key: CFStringRef,
    key_name: &'static str,
    errors: &mut Vec<String>,
) -> Option<u32> {
    let value = copy_resource_value(url, key, key_name, errors)?;
    if unsafe { CFGetTypeID(value.as_CFTypeRef()) } != unsafe { CFNumberGetTypeID() } {
        return None;
    }
    let mut percent = 0.0f64;
    let copied = unsafe {
        CFNumberGetValue(
            value.as_CFTypeRef() as CFNumberRef,
            kCFNumberDoubleType,
            &mut percent as *mut f64 as *mut c_void,
        )
    };
    if !copied || !percent.is_finite() {
        return None;
    }
    let bounded = percent.clamp(0.0, 100.0);
    Some((bounded * 1_000.0).round() as u32)
}

fn copy_unsigned_size(
    url: CFURLRef,
    key: CFStringRef,
    key_name: &'static str,
    errors: &mut Vec<String>,
) -> Option<u64> {
    let value = copy_resource_value(url, key, key_name, errors)?;
    if unsafe { CFGetTypeID(value.as_CFTypeRef()) } != unsafe { CFNumberGetTypeID() } {
        return None;
    }
    let mut size = 0i64;
    let copied = unsafe {
        CFNumberGetValue(
            value.as_CFTypeRef() as CFNumberRef,
            kCFNumberSInt64Type,
            &mut size as *mut i64 as *mut c_void,
        )
    };
    if copied && size >= 0 {
        Some(size as u64)
    } else {
        None
    }
}

fn copy_error(
    url: CFURLRef,
    key: CFStringRef,
    key_name: &'static str,
    errors: &mut Vec<String>,
) -> Option<NativeUbiquitousError> {
    let value = copy_resource_value(url, key, key_name, errors)?;
    let object = value.as_CFTypeRef() as *mut Object;
    Some(NativeUbiquitousError {
        code: unsafe { ns_error_code(object) },
        description: unsafe { ns_error_description(object) },
    })
}

fn copy_resource_value(
    url: CFURLRef,
    key: CFStringRef,
    key_name: &'static str,
    errors: &mut Vec<String>,
) -> Option<CFType> {
    let mut value: CFTypeRef = ptr::null();
    let mut error: CFErrorRef = ptr::null_mut();
    let copied = unsafe { CFURLCopyResourcePropertyForKey(url, key, &mut value, &mut error) };
    if copied == 0 || value.is_null() {
        if !error.is_null() {
            let description = unsafe { ns_error_description(error as *mut Object) }
                .unwrap_or_else(|| "resource value unavailable".to_string());
            let _error = unsafe { CFType::wrap_under_create_rule(error as CFTypeRef) };
            errors.push(format!("{}={}", key_name, description));
        }
        None
    } else {
        Some(unsafe { CFType::wrap_under_create_rule(value) })
    }
}

fn unavailable_resource_values_reason(path: &Path, errors: &[String]) -> String {
    let details = if errors.is_empty() {
        "no resource values returned".to_string()
    } else {
        errors.join("; ")
    };
    format!(
        "native FileProvider URL resource values unavailable for {}: {}",
        path.display(),
        details
    )
}

fn resource_status_for_values(has_values: bool, errors: &[String]) -> NativeFileProviderStatus {
    if !has_values && !errors.is_empty() {
        NativeFileProviderStatus::Unavailable
    } else {
        NativeFileProviderStatus::Available
    }
}

pub fn start_downloading_ubiquitous_item(path: &Path) -> NativeFileProviderOperationResult {
    run_filemanager_url_operation(
        path,
        NativeUbiquitousOperation::Download,
        "start downloading ubiquitous item",
    )
}

pub fn evict_ubiquitous_item(path: &Path) -> NativeFileProviderOperationResult {
    run_filemanager_url_operation(
        path,
        NativeUbiquitousOperation::Evict,
        "evict ubiquitous item",
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeUbiquitousOperation {
    Download,
    Evict,
}

fn run_filemanager_url_operation(
    path: &Path,
    operation: NativeUbiquitousOperation,
    label: &'static str,
) -> NativeFileProviderOperationResult {
    let url = match existing_path_url(path, "path") {
        NativePathUrl::Ready(url) => url,
        NativePathUrl::Missing(reason) => {
            return operation_result(
                NativeFileProviderOperationStatus::Missing,
                format!("{label} failed because {reason}"),
            );
        }
        NativePathUrl::Unavailable(reason) => {
            let reason = reason.replace(
                "path existence unavailable",
                "path existence is unavailable",
            );
            return operation_result(
                NativeFileProviderOperationStatus::Unavailable,
                format!("{label} failed because {reason}"),
            );
        }
        NativePathUrl::Invalid(reason) => {
            return operation_result(
                NativeFileProviderOperationStatus::UnsupportedPath,
                format!("{label} failed because {reason}"),
            );
        }
    };

    let Some(filemanager_class) = Class::get("NSFileManager") else {
        return operation_result(
            NativeFileProviderOperationStatus::Unavailable,
            "NSFileManager class is unavailable",
        );
    };
    let default_manager = unsafe { default_filemanager(filemanager_class) };
    if default_manager.is_null() {
        return operation_result(
            NativeFileProviderOperationStatus::Unavailable,
            "NSFileManager defaultManager is unavailable",
        );
    }
    let selector = operation.selector();
    if !object_responds_to_selector(default_manager, selector) {
        return operation_result(
            NativeFileProviderOperationStatus::Unavailable,
            format!("NSFileManager {} is unavailable", operation.selector_name()),
        );
    }

    let mut error: *mut c_void = ptr::null_mut();
    let ns_url = url.as_concrete_TypeRef() as *mut Object;
    let completed =
        unsafe { run_ubiquitous_operation(default_manager, selector, ns_url, &mut error) };
    if completed != 0 {
        NativeFileProviderOperationResult {
            status: NativeFileProviderOperationStatus::Completed,
            reason: None,
        }
    } else {
        let error = error as *mut Object;
        let error_code = unsafe { ns_error_code(error) };
        let error_description = unsafe { ns_error_description(error) };
        let status = operation_failure_status(error_code, error_description.as_deref());
        operation_result(
            status,
            operation_failure_reason(label, path, error_code, error_description.as_deref()),
        )
    }
}

impl NativeUbiquitousOperation {
    fn selector(self) -> Sel {
        Sel::register(self.selector_name())
    }

    const fn selector_name(self) -> &'static str {
        match self {
            Self::Download => "startDownloadingUbiquitousItemAtURL:error:",
            Self::Evict => "evictUbiquitousItemAtURL:error:",
        }
    }
}

unsafe fn default_filemanager(class: &Class) -> *mut Object {
    let send: unsafe extern "C" fn(&Class, Sel) -> *mut Object =
        std::mem::transmute(objc_msgSend as *const ());
    send(class, Sel::register("defaultManager"))
}

unsafe fn run_ubiquitous_operation(
    filemanager: *mut Object,
    selector: Sel,
    url: *mut Object,
    error: *mut *mut c_void,
) -> i8 {
    let send: unsafe extern "C" fn(*mut Object, Sel, *mut Object, *mut *mut c_void) -> i8 =
        std::mem::transmute(objc_msgSend as *const ());
    send(filemanager, selector, url, error)
}

fn operation_failure_reason(
    label: &str,
    path: &Path,
    error_code: Option<i64>,
    error_description: Option<&str>,
) -> String {
    let base = format!("{label} returned false for {}", path.display());
    match (error_code, error_description) {
        (Some(code), Some(description)) if !description.is_empty() => {
            format!("{base}: NSError {code}: {description}")
        }
        (Some(code), _) => format!("{base}: NSError {code}"),
        (None, Some(description)) if !description.is_empty() => {
            format!("{base}: {description}")
        }
        _ => base,
    }
}

fn operation_failure_status(
    error_code: Option<i64>,
    error_description: Option<&str>,
) -> NativeFileProviderOperationStatus {
    const NS_USER_CANCELLED_ERROR: i64 = 3072;
    let description = error_description.map(str::to_ascii_lowercase);
    let permission_description = description.as_deref().is_some_and(|description| {
        description.contains("permission")
            || description.contains("denied")
            || description.contains("not permitted")
    });
    let cancelled_description = description.as_deref().is_some_and(|description| {
        description.contains("cancelled")
            || description.contains("canceled")
            || description.contains("user cancel")
    });
    if error_code == Some(257) || permission_description {
        NativeFileProviderOperationStatus::PermissionDenied
    } else if error_code == Some(NS_USER_CANCELLED_ERROR) || cancelled_description {
        NativeFileProviderOperationStatus::Cancelled
    } else {
        NativeFileProviderOperationStatus::Failed
    }
}

unsafe fn class_responds_to_selector(class: &Class, selector: Sel) -> bool {
    let send: unsafe extern "C" fn(&Class, Sel, Sel) -> i8 =
        std::mem::transmute(objc_msgSend as *const ());
    send(class, Sel::register("respondsToSelector:"), selector) != 0
}

unsafe fn get_domains_with_completion_handler(class: &Class, block: *mut Object) {
    let send: unsafe extern "C" fn(&Class, Sel, *mut Object) =
        std::mem::transmute(objc_msgSend as *const ());
    send(
        class,
        Sel::register("getDomainsWithCompletionHandler:"),
        block,
    )
}

unsafe fn get_identifier_for_user_visible_file(
    class: &Class,
    url: *mut Object,
    block: *mut Object,
) {
    let send: unsafe extern "C" fn(&Class, Sel, *mut Object, *mut Object) =
        std::mem::transmute(objc_msgSend as *const ());
    send(
        class,
        Sel::register("getIdentifierForUserVisibleFileAtURL:completionHandler:"),
        url,
        block,
    )
}

unsafe fn ns_string_to_string(object: *mut Object) -> Option<String> {
    if object.is_null() {
        return None;
    }
    let send: unsafe extern "C" fn(*mut Object, Sel) -> *const c_char =
        std::mem::transmute(objc_msgSend as *const ());
    let bytes = send(object, Sel::register("UTF8String"));
    if bytes.is_null() {
        None
    } else {
        Some(
            std::ffi::CStr::from_ptr(bytes)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

unsafe fn ns_error_description(error: *mut Object) -> Option<String> {
    if error.is_null() {
        return None;
    }
    let send: unsafe extern "C" fn(*mut Object, Sel) -> *mut Object =
        std::mem::transmute(objc_msgSend as *const ());
    ns_string_to_string(send(error, Sel::register("localizedDescription")))
}

unsafe fn ns_error_code(error: *mut Object) -> Option<i64> {
    if error.is_null() {
        return None;
    }
    let send: unsafe extern "C" fn(*mut Object, Sel) -> isize =
        std::mem::transmute(objc_msgSend as *const ());
    Some(send(error, Sel::register("code")) as i64)
}

fn identity_result(
    status: NativeFileProviderIdentityStatus,
    reason: impl Into<String>,
) -> NativeFileProviderIdentity {
    NativeFileProviderIdentity {
        status,
        item_identifier: None,
        domain_identifier: None,
        reason: Some(reason.into()),
    }
}

fn operation_result(
    status: NativeFileProviderOperationStatus,
    reason: impl Into<String>,
) -> NativeFileProviderOperationResult {
    NativeFileProviderOperationResult {
        status,
        reason: Some(reason.into()),
    }
}

fn domain_enumeration(
    status: NativeFileProviderDomainStatus,
    domains: Vec<NativeFileProviderDomain>,
    reason: impl Into<String>,
) -> NativeFileProviderDomainEnumeration {
    NativeFileProviderDomainEnumeration {
        status,
        domains,
        reason: Some(reason.into()),
    }
}

fn missing_values(reason: String) -> NativeFileProviderResourceValues {
    NativeFileProviderResourceValues {
        is_ubiquitous: None,
        has_unresolved_conflicts: None,
        is_downloaded: None,
        is_downloading: None,
        is_uploading: None,
        is_uploaded: None,
        download_requested: None,
        percent_downloaded_milli: None,
        percent_uploaded_milli: None,
        downloading_status: None,
        downloading_error: None,
        uploading_error: None,
        is_excluded_from_sync: None,
        file_size_bytes: None,
        file_allocated_size_bytes: None,
        total_file_allocated_size_bytes: None,
        status: NativeFileProviderStatus::Missing,
        reason: Some(reason),
    }
}

fn unavailable_values(reason: String) -> NativeFileProviderResourceValues {
    NativeFileProviderResourceValues {
        is_ubiquitous: None,
        has_unresolved_conflicts: None,
        is_downloaded: None,
        is_downloading: None,
        is_uploading: None,
        is_uploaded: None,
        download_requested: None,
        percent_downloaded_milli: None,
        percent_uploaded_milli: None,
        downloading_status: None,
        downloading_error: None,
        uploading_error: None,
        is_excluded_from_sync: None,
        file_size_bytes: None,
        file_allocated_size_bytes: None,
        total_file_allocated_size_bytes: None,
        status: NativeFileProviderStatus::Unavailable,
        reason: Some(reason),
    }
}

fn unsupported(reason: String) -> NativeFileProviderResourceValues {
    NativeFileProviderResourceValues {
        is_ubiquitous: None,
        has_unresolved_conflicts: None,
        is_downloaded: None,
        is_downloading: None,
        is_uploading: None,
        is_uploaded: None,
        download_requested: None,
        percent_downloaded_milli: None,
        percent_uploaded_milli: None,
        downloading_status: None,
        downloading_error: None,
        uploading_error: None,
        is_excluded_from_sync: None,
        file_size_bytes: None,
        file_allocated_size_bytes: None,
        total_file_allocated_size_bytes: None,
        status: NativeFileProviderStatus::UnsupportedPath,
        reason: Some(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::ffi::OsString;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;
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
        assert_ne!(values.is_excluded_from_sync, Some(true));
        assert!(values
            .percent_downloaded_milli
            .is_none_or(|value| value <= 100_000));
        assert!(values
            .percent_uploaded_milli
            .is_none_or(|value| value <= 100_000));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn reports_native_fileprovider_identity_for_local_file_without_claiming_provider() {
        let path = std::env::temp_dir().join(format!(
            "gfm-native-fileprovider-identity-{}.txt",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, "local").unwrap();

        let identity = copy_fileprovider_identity(&path);

        assert_ne!(identity.status, NativeFileProviderIdentityStatus::Available);
        assert!(identity.item_identifier.is_none());
        assert!(identity.domain_identifier.is_none());
        assert!(identity.reason.is_some());
        fs::remove_file(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn resource_values_surface_path_probe_errors_as_unavailable() {
        let path = invalid_path("gfm-native-fileprovider-invalid");

        let values = copy_fileprovider_resource_values(&path);

        assert_eq!(values.status, NativeFileProviderStatus::Unavailable);
        assert!(values
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("path existence unavailable"));
    }

    #[cfg(unix)]
    #[test]
    fn identity_surfaces_path_probe_errors_as_unavailable() {
        let path = invalid_path("gfm-native-fileprovider-identity-invalid");

        let identity = copy_fileprovider_identity(&path);

        assert_eq!(
            identity.status,
            NativeFileProviderIdentityStatus::Unavailable
        );
        assert!(identity.item_identifier.is_none());
        assert!(identity.domain_identifier.is_none());
        assert!(identity
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("path existence unavailable"));
    }

    #[test]
    fn refuses_native_fileprovider_operation_for_missing_path() {
        let path = std::env::temp_dir().join(format!(
            "gfm-native-fileprovider-missing-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        let result = start_downloading_ubiquitous_item(&path);

        assert_eq!(result.status, NativeFileProviderOperationStatus::Missing);
        assert!(result.reason.unwrap().contains("does not exist"));
    }

    #[cfg(unix)]
    #[test]
    fn operation_surfaces_path_probe_errors_as_unavailable() {
        let path = invalid_path("gfm-native-fileprovider-operation-invalid");

        let result = start_downloading_ubiquitous_item(&path);

        assert_eq!(
            result.status,
            NativeFileProviderOperationStatus::Unavailable
        );
        assert!(result
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("path existence is unavailable"));
    }

    #[test]
    fn formats_native_fileprovider_operation_error_details() {
        let path = Path::new("/tmp/Remote.icloud");

        let reason = operation_failure_reason(
            "start downloading ubiquitous item",
            path,
            Some(257),
            Some("Operation not permitted"),
        );

        assert_eq!(
            reason,
            "start downloading ubiquitous item returned false for /tmp/Remote.icloud: NSError 257: Operation not permitted"
        );
    }

    #[test]
    fn classifies_native_fileprovider_operation_failures() {
        assert_eq!(
            operation_failure_status(Some(257), Some("Operation not permitted")),
            NativeFileProviderOperationStatus::PermissionDenied
        );
        assert_eq!(
            operation_failure_status(None, Some("Permission denied")),
            NativeFileProviderOperationStatus::PermissionDenied
        );
        assert_eq!(
            operation_failure_status(Some(3072), Some("The operation was cancelled.")),
            NativeFileProviderOperationStatus::Cancelled
        );
        assert_eq!(
            operation_failure_status(None, Some("User canceled operation")),
            NativeFileProviderOperationStatus::Cancelled
        );
        assert_eq!(
            operation_failure_status(Some(5), Some("Input/output error")),
            NativeFileProviderOperationStatus::Failed
        );
    }

    #[test]
    fn native_resource_status_reports_unavailable_only_when_all_values_fail() {
        let errors = vec!["NSURLIsUbiquitousItemKey=Operation not permitted".to_string()];

        assert_eq!(
            resource_status_for_values(false, &errors),
            NativeFileProviderStatus::Unavailable
        );
        assert_eq!(
            resource_status_for_values(true, &errors),
            NativeFileProviderStatus::Available
        );
        assert_eq!(
            unavailable_resource_values_reason(Path::new("/tmp/Remote.icloud"), &errors),
            "native FileProvider URL resource values unavailable for /tmp/Remote.icloud: NSURLIsUbiquitousItemKey=Operation not permitted"
        );
        assert_eq!(
            NativeFileProviderStatus::Unavailable.as_str(),
            "unavailable"
        );
    }

    #[test]
    fn enumerates_native_fileprovider_domains_or_reports_host_state() {
        let domains = enumerate_fileprovider_domains();

        assert!(matches!(
            domains.status,
            NativeFileProviderDomainStatus::Available
                | NativeFileProviderDomainStatus::NoDomains
                | NativeFileProviderDomainStatus::Unsupported
                | NativeFileProviderDomainStatus::PermissionDenied
                | NativeFileProviderDomainStatus::Unavailable
        ));
        if domains.status == NativeFileProviderDomainStatus::Available {
            assert!(!domains.domains.is_empty());
        }
    }

    #[cfg(unix)]
    fn invalid_path(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(OsString::from_vec(
            format!("/tmp/{name}\0path").into_bytes(),
        ))
    }
}
