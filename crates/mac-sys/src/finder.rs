use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use core_foundation::url::CFURL;
use core_foundation_sys::base::OSStatus;
use core_foundation_sys::string::CFStringRef;
use std::path::Path;
use std::ptr;

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn LSCopyKindStringForURL(
        url: core_foundation_sys::url::CFURLRef,
        out_kind: *mut CFStringRef,
    ) -> OSStatus;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeFinderKind {
    pub kind: Option<String>,
    pub status: NativeFinderKindStatus,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeFinderKindStatus {
    Available,
    Unavailable,
}

pub fn copy_kind_string_for_path(path: &Path) -> NativeFinderKind {
    let Some(url) = CFURL::from_path(path, path.is_dir()) else {
        return unavailable(format!("invalid path URL: {}", path.display()));
    };

    let mut kind: CFStringRef = ptr::null();
    let status = unsafe { LSCopyKindStringForURL(url.as_concrete_TypeRef(), &mut kind) };
    if status != 0 {
        return unavailable(format!(
            "LSCopyKindStringForURL failed for {} with OSStatus {status}",
            path.display()
        ));
    }
    if kind.is_null() {
        return unavailable(format!(
            "LSCopyKindStringForURL returned no kind for {}",
            path.display()
        ));
    }

    let kind = unsafe { CFString::wrap_under_create_rule(kind) }.to_string();
    if kind.trim().is_empty() {
        unavailable(format!(
            "LSCopyKindStringForURL returned an empty kind for {}",
            path.display()
        ))
    } else {
        NativeFinderKind {
            kind: Some(kind),
            status: NativeFinderKindStatus::Available,
            reason: None,
        }
    }
}

fn unavailable(reason: String) -> NativeFinderKind {
    NativeFinderKind {
        kind: None,
        status: NativeFinderKindStatus::Unavailable,
        reason: Some(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn resolves_native_kind_for_regular_file() {
        let path = std::env::temp_dir().join(format!(
            "gfm-native-finder-kind-{}.txt",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, "kind").unwrap();

        let kind = copy_kind_string_for_path(&path);

        assert_eq!(kind.status, NativeFinderKindStatus::Available);
        assert!(!kind.kind.as_deref().unwrap().trim().is_empty());
        fs::remove_file(path).unwrap();
    }
}
