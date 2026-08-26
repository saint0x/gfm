use core_foundation::base::{kCFAllocatorDefault, TCFType};
use core_foundation::data::CFData;
use core_foundation::url::CFURL;
use core_foundation_sys::base::{Boolean, CFAllocatorRef, CFOptionFlags};
use core_foundation_sys::error::CFErrorRef;
use core_foundation_sys::url::CFURLRef;
use std::path::{Path, PathBuf};
use std::ptr;

const BOOKMARK_CREATION_WITH_SECURITY_SCOPE: CFOptionFlags = 1 << 11;
const BOOKMARK_CREATION_SECURITY_SCOPE_READ_ONLY: CFOptionFlags = 1 << 12;
const BOOKMARK_RESOLUTION_WITHOUT_UI: CFOptionFlags = 1 << 8;
const BOOKMARK_RESOLUTION_WITHOUT_MOUNTING: CFOptionFlags = 1 << 9;
const BOOKMARK_RESOLUTION_WITH_SECURITY_SCOPE: CFOptionFlags = 1 << 10;
const BOOKMARK_RESOLUTION_WITHOUT_IMPLICIT_START: CFOptionFlags = 1 << 15;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFURLCreateBookmarkData(
        allocator: CFAllocatorRef,
        url: CFURLRef,
        options: CFOptionFlags,
        resource_properties_to_include: *const core::ffi::c_void,
        relative_to_url: CFURLRef,
        error: *mut CFErrorRef,
    ) -> core_foundation_sys::data::CFDataRef;
    fn CFURLCreateByResolvingBookmarkData(
        allocator: CFAllocatorRef,
        bookmark: core_foundation_sys::data::CFDataRef,
        options: CFOptionFlags,
        relative_to_url: CFURLRef,
        resource_properties_to_include: *const core::ffi::c_void,
        is_stale: *mut Boolean,
        error: *mut CFErrorRef,
    ) -> CFURLRef;
    fn CFURLStartAccessingSecurityScopedResource(url: CFURLRef) -> Boolean;
    fn CFURLStopAccessingSecurityScopedResource(url: CFURLRef);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeBookmarkData {
    pub status: NativeBookmarkStatus,
    pub data: Vec<u8>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeBookmarkResolution {
    pub status: NativeBookmarkStatus,
    pub path: Option<PathBuf>,
    pub stale: bool,
    pub access_started: bool,
    pub reason: Option<String>,
}

pub struct NativeSecurityScopedAccess {
    url: CFURL,
    pub path: Option<PathBuf>,
    pub stale: bool,
}

impl Drop for NativeSecurityScopedAccess {
    fn drop(&mut self) {
        unsafe { CFURLStopAccessingSecurityScopedResource(self.url.as_concrete_TypeRef()) };
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeBookmarkStatus {
    Available,
    Missing,
    Unavailable,
}

pub fn create_security_scoped_bookmark(path: &Path, read_only: bool) -> NativeBookmarkData {
    if !path.exists() {
        return missing_data(format!("bookmark path does not exist: {}", path.display()));
    }
    let Some(url) = CFURL::from_path(path, path.is_dir()) else {
        return unavailable_data(format!("invalid bookmark path URL: {}", path.display()));
    };

    let mut options = BOOKMARK_CREATION_WITH_SECURITY_SCOPE;
    if read_only {
        options |= BOOKMARK_CREATION_SECURITY_SCOPE_READ_ONLY;
    }
    let mut error = ptr::null_mut();
    let data = unsafe {
        CFURLCreateBookmarkData(
            kCFAllocatorDefault,
            url.as_concrete_TypeRef(),
            options,
            ptr::null(),
            ptr::null(),
            &mut error,
        )
    };
    if data.is_null() {
        return unavailable_data(format!(
            "CoreFoundation did not create security-scoped bookmark data for {}",
            path.display()
        ));
    }

    let data = unsafe { CFData::wrap_under_create_rule(data) };
    NativeBookmarkData {
        status: NativeBookmarkStatus::Available,
        data: data.bytes().to_vec(),
        reason: None,
    }
}

pub fn start_security_scoped_bookmark_access(
    data: &[u8],
) -> std::result::Result<NativeSecurityScopedAccess, NativeBookmarkResolution> {
    let resolution = resolve_security_scoped_bookmark_url(data)?;
    let access_started =
        unsafe { CFURLStartAccessingSecurityScopedResource(resolution.url.as_concrete_TypeRef()) };
    if access_started == 0 {
        return Err(NativeBookmarkResolution {
            status: NativeBookmarkStatus::Unavailable,
            path: resolution.path,
            stale: resolution.stale,
            access_started: false,
            reason: Some("CoreFoundation did not start security-scoped access".to_string()),
        });
    }
    Ok(NativeSecurityScopedAccess {
        url: resolution.url,
        path: resolution.path,
        stale: resolution.stale,
    })
}

pub fn resolve_security_scoped_bookmark(
    data: &[u8],
    start_access: bool,
) -> NativeBookmarkResolution {
    let resolution = match resolve_security_scoped_bookmark_url(data) {
        Ok(resolution) => resolution,
        Err(resolution) => return resolution,
    };
    let access_started = start_access
        && unsafe {
            CFURLStartAccessingSecurityScopedResource(resolution.url.as_concrete_TypeRef()) != 0
        };
    if start_access && !access_started {
        return NativeBookmarkResolution {
            status: NativeBookmarkStatus::Unavailable,
            path: resolution.path,
            stale: resolution.stale,
            access_started: false,
            reason: Some("CoreFoundation did not start security-scoped access".to_string()),
        };
    }
    if access_started {
        unsafe { CFURLStopAccessingSecurityScopedResource(resolution.url.as_concrete_TypeRef()) };
    }

    NativeBookmarkResolution {
        status: NativeBookmarkStatus::Available,
        path: resolution.path,
        stale: resolution.stale,
        access_started,
        reason: None,
    }
}

struct ResolvedBookmarkUrl {
    url: CFURL,
    path: Option<PathBuf>,
    stale: bool,
}

fn resolve_security_scoped_bookmark_url(
    data: &[u8],
) -> std::result::Result<ResolvedBookmarkUrl, NativeBookmarkResolution> {
    if data.is_empty() {
        return Err(unavailable_resolution("bookmark data is empty"));
    }
    let data = CFData::from_buffer(data);
    let mut stale = 0;
    let mut error = ptr::null_mut();
    let url = unsafe {
        CFURLCreateByResolvingBookmarkData(
            kCFAllocatorDefault,
            data.as_concrete_TypeRef(),
            BOOKMARK_RESOLUTION_WITHOUT_UI
                | BOOKMARK_RESOLUTION_WITHOUT_MOUNTING
                | BOOKMARK_RESOLUTION_WITH_SECURITY_SCOPE
                | BOOKMARK_RESOLUTION_WITHOUT_IMPLICIT_START,
            ptr::null(),
            ptr::null(),
            &mut stale,
            &mut error,
        )
    };
    if url.is_null() {
        return Err(unavailable_resolution(
            "CoreFoundation did not resolve security-scoped bookmark data",
        ));
    }

    let url = unsafe { CFURL::wrap_under_create_rule(url) };
    let path = url.to_path();
    Ok(ResolvedBookmarkUrl {
        url,
        path,
        stale: stale != 0,
    })
}

fn missing_data(reason: impl Into<String>) -> NativeBookmarkData {
    NativeBookmarkData {
        status: NativeBookmarkStatus::Missing,
        data: Vec::new(),
        reason: Some(reason.into()),
    }
}

fn unavailable_data(reason: impl Into<String>) -> NativeBookmarkData {
    NativeBookmarkData {
        status: NativeBookmarkStatus::Unavailable,
        data: Vec::new(),
        reason: Some(reason.into()),
    }
}

fn unavailable_resolution(reason: impl Into<String>) -> NativeBookmarkResolution {
    NativeBookmarkResolution {
        status: NativeBookmarkStatus::Unavailable,
        path: None,
        stale: false,
        access_started: false,
        reason: Some(reason.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn reports_missing_bookmark_targets_without_native_call() {
        let path = std::env::temp_dir().join("gfm-security-bookmark-missing-target");
        let bookmark = create_security_scoped_bookmark(&path, true);

        assert_eq!(bookmark.status, NativeBookmarkStatus::Missing);
        assert!(bookmark.data.is_empty());
    }

    #[test]
    fn rejects_empty_bookmark_data() {
        let resolution = resolve_security_scoped_bookmark(&[], false);

        assert_eq!(resolution.status, NativeBookmarkStatus::Unavailable);
        assert!(resolution.reason.unwrap().contains("empty"));
    }

    #[test]
    fn creates_and_resolves_security_scoped_bookmark_for_regular_file() {
        let path = std::env::temp_dir().join(format!(
            "gfm-security-bookmark-{}.txt",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, "bookmark").unwrap();

        let bookmark = create_security_scoped_bookmark(&path, true);

        assert_eq!(bookmark.status, NativeBookmarkStatus::Available);
        assert!(!bookmark.data.is_empty());

        let resolution = resolve_security_scoped_bookmark(&bookmark.data, false);

        assert_eq!(resolution.status, NativeBookmarkStatus::Available);
        assert_eq!(
            resolution
                .path
                .as_ref()
                .and_then(|path| path.canonicalize().ok()),
            Some(path.canonicalize().unwrap())
        );
        assert!(!resolution.stale);

        fs::remove_file(path).unwrap();
    }
}
