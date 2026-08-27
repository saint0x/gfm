use block::ConcreteBlock;
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::{CFBoolean, CFBooleanRef};
use core_foundation::string::{CFString, CFStringRef};
use core_foundation::url::CFURL;
use core_foundation_sys::base::{Boolean, CFGetTypeID, CFTypeRef};
use core_foundation_sys::error::CFErrorRef;
use core_foundation_sys::number::{
    kCFNumberDoubleType, CFNumberGetTypeID, CFNumberGetValue, CFNumberRef,
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
    static NSURLUbiquitousItemIsDownloadingKey: CFStringRef;
    static NSURLUbiquitousItemIsUploadingKey: CFStringRef;
    static NSURLUbiquitousItemIsUploadedKey: CFStringRef;
    static NSURLUbiquitousItemDownloadRequestedKey: CFStringRef;
    static NSURLUbiquitousItemPercentDownloadedKey: CFStringRef;
    static NSURLUbiquitousItemPercentUploadedKey: CFStringRef;
    static NSURLUbiquitousItemDownloadingStatusKey: CFStringRef;
    static NSURLUbiquitousItemDownloadingErrorKey: CFStringRef;
    static NSURLUbiquitousItemUploadingErrorKey: CFStringRef;
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
    pub is_downloading: Option<bool>,
    pub is_uploading: Option<bool>,
    pub is_uploaded: Option<bool>,
    pub download_requested: Option<bool>,
    pub percent_downloaded_milli: Option<u32>,
    pub percent_uploaded_milli: Option<u32>,
    pub downloading_status: Option<NativeUbiquitousDownloadingStatus>,
    pub downloading_error: Option<NativeUbiquitousError>,
    pub uploading_error: Option<NativeUbiquitousError>,
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
}

impl NativeFileProviderStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Missing => "missing",
            Self::UnsupportedPath => "unsupported-path",
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
    if !path.exists() {
        return missing_values(format!(
            "fileprovider path does not exist: {}",
            path.display()
        ));
    }
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
    let download_requested = copy_bool(url.as_concrete_TypeRef(), unsafe {
        NSURLUbiquitousItemDownloadRequestedKey
    });
    let percent_downloaded_milli = copy_percent_milli(url.as_concrete_TypeRef(), unsafe {
        NSURLUbiquitousItemPercentDownloadedKey
    });
    let percent_uploaded_milli = copy_percent_milli(url.as_concrete_TypeRef(), unsafe {
        NSURLUbiquitousItemPercentUploadedKey
    });
    let downloading_status = copy_downloading_status(url.as_concrete_TypeRef());
    let downloading_error = copy_error(url.as_concrete_TypeRef(), unsafe {
        NSURLUbiquitousItemDownloadingErrorKey
    });
    let uploading_error = copy_error(url.as_concrete_TypeRef(), unsafe {
        NSURLUbiquitousItemUploadingErrorKey
    });

    NativeFileProviderResourceValues {
        is_ubiquitous,
        has_unresolved_conflicts,
        is_downloading,
        is_uploading,
        is_uploaded,
        download_requested,
        percent_downloaded_milli,
        percent_uploaded_milli,
        downloading_status,
        downloading_error,
        uploading_error,
        status: NativeFileProviderStatus::Available,
        reason: None,
    }
}

pub fn copy_fileprovider_identity(path: &Path) -> NativeFileProviderIdentity {
    if !path.exists() {
        return identity_result(
            NativeFileProviderIdentityStatus::Missing,
            format!(
                "fileprovider identity path does not exist: {}",
                path.display()
            ),
        );
    }
    let Some(url) = CFURL::from_path(path, path.is_dir()) else {
        return identity_result(
            NativeFileProviderIdentityStatus::UnsupportedPath,
            format!("invalid path URL: {}", path.display()),
        );
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

fn copy_percent_milli(url: CFURLRef, key: CFStringRef) -> Option<u32> {
    let value = copy_resource_value(url, key)?;
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

fn copy_error(url: CFURLRef, key: CFStringRef) -> Option<NativeUbiquitousError> {
    let value = copy_resource_value(url, key)?;
    let object = value.as_CFTypeRef() as *mut Object;
    Some(NativeUbiquitousError {
        code: unsafe { ns_error_code(object) },
        description: unsafe { ns_error_description(object) },
    })
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
    if !path.exists() {
        return operation_result(
            NativeFileProviderOperationStatus::Missing,
            format!(
                "{label} failed because path does not exist: {}",
                path.display()
            ),
        );
    }
    let Some(url) = CFURL::from_path(path, path.is_dir()) else {
        return operation_result(
            NativeFileProviderOperationStatus::UnsupportedPath,
            format!(
                "{label} failed because path URL is invalid: {}",
                path.display()
            ),
        );
    };

    let Some(filemanager_class) = Class::get("NSFileManager") else {
        return operation_result(
            NativeFileProviderOperationStatus::Failed,
            "NSFileManager class is unavailable",
        );
    };
    let default_manager = unsafe { default_filemanager(filemanager_class) };
    if default_manager.is_null() {
        return operation_result(
            NativeFileProviderOperationStatus::Failed,
            "NSFileManager defaultManager is unavailable",
        );
    }

    let mut error: *mut c_void = ptr::null_mut();
    let ns_url = url.as_concrete_TypeRef() as *mut Object;
    let completed =
        unsafe { run_ubiquitous_operation(default_manager, operation, ns_url, &mut error) };
    if completed != 0 {
        NativeFileProviderOperationResult {
            status: NativeFileProviderOperationStatus::Completed,
            reason: None,
        }
    } else {
        operation_result(
            NativeFileProviderOperationStatus::Failed,
            format!("{label} returned false for {}", path.display()),
        )
    }
}

unsafe fn default_filemanager(class: &Class) -> *mut Object {
    let send: unsafe extern "C" fn(&Class, Sel) -> *mut Object =
        std::mem::transmute(objc_msgSend as *const ());
    send(class, Sel::register("defaultManager"))
}

unsafe fn run_ubiquitous_operation(
    filemanager: *mut Object,
    operation: NativeUbiquitousOperation,
    url: *mut Object,
    error: *mut *mut c_void,
) -> i8 {
    let send: unsafe extern "C" fn(*mut Object, Sel, *mut Object, *mut *mut c_void) -> i8 =
        std::mem::transmute(objc_msgSend as *const ());
    let selector = match operation {
        NativeUbiquitousOperation::Download => {
            Sel::register("startDownloadingUbiquitousItemAtURL:error:")
        }
        NativeUbiquitousOperation::Evict => Sel::register("evictUbiquitousItemAtURL:error:"),
    };
    send(filemanager, selector, url, error)
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
        is_downloading: None,
        is_uploading: None,
        is_uploaded: None,
        download_requested: None,
        percent_downloaded_milli: None,
        percent_uploaded_milli: None,
        downloading_status: None,
        downloading_error: None,
        uploading_error: None,
        status: NativeFileProviderStatus::Missing,
        reason: Some(reason),
    }
}

fn unsupported(reason: String) -> NativeFileProviderResourceValues {
    NativeFileProviderResourceValues {
        is_ubiquitous: None,
        has_unresolved_conflicts: None,
        is_downloading: None,
        is_uploading: None,
        is_uploaded: None,
        download_requested: None,
        percent_downloaded_milli: None,
        percent_uploaded_milli: None,
        downloading_status: None,
        downloading_error: None,
        uploading_error: None,
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
}
