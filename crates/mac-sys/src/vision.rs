use crate::url::{existing_path_url, NativePathUrl};
use core_foundation::base::TCFType;
use objc::runtime::{Class, Object, Sel};
use std::path::Path;
use std::ptr;

#[link(name = "Vision", kind = "framework")]
extern "C" {}

#[link(name = "objc")]
extern "C" {
    fn objc_msgSend();
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeTextRecognition {
    pub lines: Vec<String>,
    pub status: NativeTextRecognitionStatus,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeTextRecognitionStatus {
    Available,
    Empty,
    Missing,
    Unsupported,
    Failed,
    Unavailable,
}

impl NativeTextRecognitionStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Empty => "empty",
            Self::Missing => "missing",
            Self::Unsupported => "unsupported",
            Self::Failed => "failed",
            Self::Unavailable => "unavailable",
        }
    }
}

pub fn recognize_text_for_image(path: &Path) -> NativeTextRecognition {
    let url = match existing_path_url(path, "vision text recognition image") {
        NativePathUrl::Ready(url) => url,
        NativePathUrl::Missing(reason) => {
            return recognition(NativeTextRecognitionStatus::Missing, Vec::new(), reason)
        }
        NativePathUrl::Unavailable(reason) => {
            return recognition(NativeTextRecognitionStatus::Unavailable, Vec::new(), reason);
        }
        NativePathUrl::Invalid(reason) => {
            return recognition(NativeTextRecognitionStatus::Unsupported, Vec::new(), reason);
        }
    };

    let Some(request_class) = Class::get("VNRecognizeTextRequest") else {
        return recognition(
            NativeTextRecognitionStatus::Unsupported,
            Vec::new(),
            "VNRecognizeTextRequest class is unavailable",
        );
    };
    let Some(handler_class) = Class::get("VNImageRequestHandler") else {
        return recognition(
            NativeTextRecognitionStatus::Unsupported,
            Vec::new(),
            "VNImageRequestHandler class is unavailable",
        );
    };
    let Some(array_class) = Class::get("NSArray") else {
        return recognition(
            NativeTextRecognitionStatus::Unavailable,
            Vec::new(),
            "NSArray class is unavailable",
        );
    };
    let Some(dictionary_class) = Class::get("NSDictionary") else {
        return recognition(
            NativeTextRecognitionStatus::Unavailable,
            Vec::new(),
            "NSDictionary class is unavailable",
        );
    };

    unsafe {
        let pool = autorelease_pool();
        let request = alloc_init(request_class);
        if request.is_null() {
            release_if_present(pool);
            return recognition(
                NativeTextRecognitionStatus::Unavailable,
                Vec::new(),
                "VNRecognizeTextRequest allocation failed",
            );
        }
        configure_text_request(request);

        let options = dictionary(dictionary_class);
        let handler_alloc = alloc(handler_class);
        let handler = init_image_request_handler(handler_alloc, url.as_concrete_TypeRef(), options);
        if handler.is_null() {
            release_if_present(request);
            release_if_present(pool);
            return recognition(
                NativeTextRecognitionStatus::Unavailable,
                Vec::new(),
                "VNImageRequestHandler allocation failed",
            );
        }

        let requests = array_with_object(array_class, request);
        let mut error: *mut Object = ptr::null_mut();
        let ok = perform_requests(handler, requests, &mut error);
        let result = if ok {
            let lines = recognized_lines(request);
            if lines.is_empty() {
                recognition(
                    NativeTextRecognitionStatus::Empty,
                    lines,
                    "Vision returned no text",
                )
            } else {
                NativeTextRecognition {
                    lines,
                    status: NativeTextRecognitionStatus::Available,
                    reason: None,
                }
            }
        } else {
            recognition(
                NativeTextRecognitionStatus::Failed,
                Vec::new(),
                ns_error_description(error).unwrap_or_else(|| {
                    format!(
                        "VNImageRequestHandler performRequests failed for {}",
                        path.display()
                    )
                }),
            )
        };

        release_if_present(handler);
        release_if_present(request);
        release_if_present(pool);
        result
    }
}

fn recognition(
    status: NativeTextRecognitionStatus,
    lines: Vec<String>,
    reason: impl Into<String>,
) -> NativeTextRecognition {
    NativeTextRecognition {
        lines,
        status,
        reason: Some(reason.into()),
    }
}

unsafe fn configure_text_request(request: *mut Object) {
    let set_level = Sel::register("setRecognitionLevel:");
    if object_responds_to_selector(request, set_level) {
        let send: unsafe extern "C" fn(*mut Object, Sel, usize) =
            std::mem::transmute(objc_msgSend as *const ());
        send(request, set_level, 0);
    }
    let set_correction = Sel::register("setUsesLanguageCorrection:");
    if object_responds_to_selector(request, set_correction) {
        let send: unsafe extern "C" fn(*mut Object, Sel, i8) =
            std::mem::transmute(objc_msgSend as *const ());
        send(request, set_correction, 1);
    }
}

unsafe fn recognized_lines(request: *mut Object) -> Vec<String> {
    let results = object_property(request, "results");
    nsarray_objects(results)
        .into_iter()
        .filter_map(|observation| {
            let top_candidates = top_candidates(observation, 1);
            nsarray_objects(top_candidates)
                .into_iter()
                .next()
                .and_then(|candidate| string_property(candidate, "string"))
        })
        .filter(|line| !line.trim().is_empty())
        .collect()
}

unsafe fn autorelease_pool() -> *mut Object {
    let Some(pool_class) = Class::get("NSAutoreleasePool") else {
        return ptr::null_mut();
    };
    alloc_init(pool_class)
}

unsafe fn alloc_init(class: &Class) -> *mut Object {
    let allocated = alloc(class);
    if allocated.is_null() {
        return ptr::null_mut();
    }
    let send: unsafe extern "C" fn(*mut Object, Sel) -> *mut Object =
        std::mem::transmute(objc_msgSend as *const ());
    send(allocated, Sel::register("init"))
}

unsafe fn alloc(class: &Class) -> *mut Object {
    let send: unsafe extern "C" fn(&Class, Sel) -> *mut Object =
        std::mem::transmute(objc_msgSend as *const ());
    send(class, Sel::register("alloc"))
}

unsafe fn dictionary(class: &Class) -> *mut Object {
    let send: unsafe extern "C" fn(&Class, Sel) -> *mut Object =
        std::mem::transmute(objc_msgSend as *const ());
    send(class, Sel::register("dictionary"))
}

unsafe fn array_with_object(class: &Class, object: *mut Object) -> *mut Object {
    let send: unsafe extern "C" fn(&Class, Sel, *mut Object) -> *mut Object =
        std::mem::transmute(objc_msgSend as *const ());
    send(class, Sel::register("arrayWithObject:"), object)
}

unsafe fn init_image_request_handler(
    handler: *mut Object,
    url: core_foundation_sys::url::CFURLRef,
    options: *mut Object,
) -> *mut Object {
    let send: unsafe extern "C" fn(
        *mut Object,
        Sel,
        core_foundation_sys::url::CFURLRef,
        *mut Object,
    ) -> *mut Object = std::mem::transmute(objc_msgSend as *const ());
    send(handler, Sel::register("initWithURL:options:"), url, options)
}

unsafe fn perform_requests(
    handler: *mut Object,
    requests: *mut Object,
    error: *mut *mut Object,
) -> bool {
    let send: unsafe extern "C" fn(*mut Object, Sel, *mut Object, *mut *mut Object) -> i8 =
        std::mem::transmute(objc_msgSend as *const ());
    send(
        handler,
        Sel::register("performRequests:error:"),
        requests,
        error,
    ) != 0
}

unsafe fn top_candidates(observation: *mut Object, limit: usize) -> *mut Object {
    let send: unsafe extern "C" fn(*mut Object, Sel, usize) -> *mut Object =
        std::mem::transmute(objc_msgSend as *const ());
    send(observation, Sel::register("topCandidates:"), limit)
}

unsafe fn object_property(object: *mut Object, selector: &str) -> *mut Object {
    if object.is_null() {
        return ptr::null_mut();
    }
    let selector = Sel::register(selector);
    if !object_responds_to_selector(object, selector) {
        return ptr::null_mut();
    }
    let send: unsafe extern "C" fn(*mut Object, Sel) -> *mut Object =
        std::mem::transmute(objc_msgSend as *const ());
    send(object, selector)
}

unsafe fn string_property(object: *mut Object, selector: &str) -> Option<String> {
    ns_string_to_string(object_property(object, selector))
}

unsafe fn nsarray_objects(array: *mut Object) -> Vec<*mut Object> {
    if array.is_null() {
        return Vec::new();
    }
    let count_send: unsafe extern "C" fn(*mut Object, Sel) -> usize =
        std::mem::transmute(objc_msgSend as *const ());
    let object_send: unsafe extern "C" fn(*mut Object, Sel, usize) -> *mut Object =
        std::mem::transmute(objc_msgSend as *const ());
    let count = count_send(array, Sel::register("count"));
    (0..count)
        .filter_map(|index| {
            let object = object_send(array, Sel::register("objectAtIndex:"), index);
            (!object.is_null()).then_some(object)
        })
        .collect()
}

unsafe fn object_responds_to_selector(object: *mut Object, selector: Sel) -> bool {
    if object.is_null() {
        return false;
    }
    let responds: unsafe extern "C" fn(*mut Object, Sel, Sel) -> i8 =
        std::mem::transmute(objc_msgSend as *const ());
    responds(object, Sel::register("respondsToSelector:"), selector) != 0
}

unsafe fn ns_string_to_string(object: *mut Object) -> Option<String> {
    if object.is_null() {
        return None;
    }
    let send: unsafe extern "C" fn(*mut Object, Sel) -> *const libc::c_char =
        std::mem::transmute(objc_msgSend as *const ());
    let bytes = send(object, Sel::register("UTF8String"));
    if bytes.is_null() {
        None
    } else {
        Some(
            std::ffi::CStr::from_ptr(bytes)
                .to_string_lossy()
                .to_string(),
        )
    }
}

unsafe fn ns_error_description(error: *mut Object) -> Option<String> {
    if error.is_null() {
        return None;
    }
    string_property(error, "localizedDescription")
}

unsafe fn release_if_present(object: *mut Object) {
    if object.is_null() {
        return;
    }
    let release: unsafe extern "C" fn(*mut Object, Sel) =
        std::mem::transmute(objc_msgSend as *const ());
    release(object, Sel::register("release"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "gfm-native-vision-{label}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn vision_text_recognition_reports_missing_path_without_requesting_vision() {
        let path = unique_path("missing");

        let report = recognize_text_for_image(&path);

        assert_eq!(report.status, NativeTextRecognitionStatus::Missing);
        assert!(report.lines.is_empty());
        assert!(report
            .reason
            .as_deref()
            .unwrap()
            .contains("vision text recognition image does not exist"));
    }

    #[test]
    fn vision_text_recognition_reports_invalid_image_as_failure_or_empty() {
        let path = unique_path("invalid");
        fs::write(&path, b"not an image").unwrap();

        let report = recognize_text_for_image(&path);

        assert!(matches!(
            report.status,
            NativeTextRecognitionStatus::Failed
                | NativeTextRecognitionStatus::Empty
                | NativeTextRecognitionStatus::Unsupported
        ));
        fs::remove_file(path).unwrap();
    }
}
