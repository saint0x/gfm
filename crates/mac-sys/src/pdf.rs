use crate::url::{existing_path_url, path_url, NativePathUrl};
use core_foundation::base::TCFType;
use objc::runtime::{Class, Object, Sel};
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr;

#[link(name = "AppKit", kind = "framework")]
extern "C" {}

#[link(name = "PDFKit", kind = "framework")]
extern "C" {}

#[link(name = "objc")]
extern "C" {
    fn objc_msgSend();
}

const PDF_DISPLAY_BOX_MEDIA_BOX: i32 = 0;
const NS_BITMAP_IMAGE_FILE_TYPE_PNG: usize = 4;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct NSPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct NSSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct NSRect {
    origin: NSPoint,
    size: NSSize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativePdfPageRasterization {
    pub pages: Vec<PathBuf>,
    pub status: NativePdfPageRasterizationStatus,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativePdfPageRasterizationStatus {
    Available,
    Empty,
    Missing,
    Unsupported,
    Failed,
    Unavailable,
}

impl NativePdfPageRasterizationStatus {
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

pub fn rasterize_pdf_pages_for_ocr(
    pdf: &Path,
    output_dir: &Path,
    max_pages: usize,
    max_dimension_px: u32,
) -> NativePdfPageRasterization {
    if max_pages == 0 {
        return rasterization(
            NativePdfPageRasterizationStatus::Unsupported,
            Vec::new(),
            "PDF OCR rasterization max_pages must be greater than zero",
        );
    }
    if max_dimension_px == 0 {
        return rasterization(
            NativePdfPageRasterizationStatus::Unsupported,
            Vec::new(),
            "PDF OCR rasterization max_dimension_px must be greater than zero",
        );
    }
    let pdf_url = match existing_path_url(pdf, "PDF OCR rasterization input") {
        NativePathUrl::Ready(url) => url,
        NativePathUrl::Missing(reason) => {
            return rasterization(
                NativePdfPageRasterizationStatus::Missing,
                Vec::new(),
                reason,
            );
        }
        NativePathUrl::Unavailable(reason) => {
            return rasterization(
                NativePdfPageRasterizationStatus::Unavailable,
                Vec::new(),
                reason,
            );
        }
        NativePathUrl::Invalid(reason) => {
            return rasterization(
                NativePdfPageRasterizationStatus::Unsupported,
                Vec::new(),
                reason,
            );
        }
    };
    if let Err(error) = fs::create_dir_all(output_dir) {
        return rasterization(
            NativePdfPageRasterizationStatus::Unavailable,
            Vec::new(),
            format!(
                "PDF OCR rasterization output directory unavailable: {}: {error}",
                output_dir.display()
            ),
        );
    }

    let Some(document_class) = Class::get("PDFDocument") else {
        return rasterization(
            NativePdfPageRasterizationStatus::Unsupported,
            Vec::new(),
            "PDFDocument class is unavailable",
        );
    };
    let Some(bitmap_class) = Class::get("NSBitmapImageRep") else {
        return rasterization(
            NativePdfPageRasterizationStatus::Unavailable,
            Vec::new(),
            "NSBitmapImageRep class is unavailable",
        );
    };
    let Some(dictionary_class) = Class::get("NSDictionary") else {
        return rasterization(
            NativePdfPageRasterizationStatus::Unavailable,
            Vec::new(),
            "NSDictionary class is unavailable",
        );
    };

    unsafe {
        let pool = autorelease_pool();
        let document_alloc = alloc(document_class);
        let document = init_pdf_document_with_url(document_alloc, pdf_url.as_concrete_TypeRef());
        if document.is_null() {
            release_if_present(pool);
            return rasterization(
                NativePdfPageRasterizationStatus::Failed,
                Vec::new(),
                format!("PDFDocument could not open {}", pdf.display()),
            );
        }

        let total_pages = page_count(document);
        if total_pages == 0 {
            release_if_present(document);
            release_if_present(pool);
            return rasterization(
                NativePdfPageRasterizationStatus::Empty,
                Vec::new(),
                format!("PDFDocument reported zero pages for {}", pdf.display()),
            );
        }

        let mut pages = Vec::new();
        let limit = total_pages.min(max_pages);
        for index in 0..limit {
            let page = page_at_index(document, index);
            if page.is_null() {
                continue;
            }
            let output = output_dir.join(format!("page-{index:06}.png"));
            let output_url = match path_url(&output, "PDF OCR rasterization output", false) {
                NativePathUrl::Ready(url) => url,
                NativePathUrl::Missing(reason)
                | NativePathUrl::Unavailable(reason)
                | NativePathUrl::Invalid(reason) => {
                    release_if_present(document);
                    release_if_present(pool);
                    return rasterization(
                        NativePdfPageRasterizationStatus::Unsupported,
                        pages,
                        reason,
                    );
                }
            };
            let size = bounded_page_size(page, max_dimension_px);
            let image = thumbnail_for_page(page, size);
            if image.is_null() {
                continue;
            }
            let Some(tiff) = image_tiff_representation(image) else {
                continue;
            };
            let bitmap = bitmap_rep_with_data(bitmap_class, tiff);
            if bitmap.is_null() {
                continue;
            }
            let properties = dictionary(dictionary_class);
            let Some(png) =
                bitmap_representation(bitmap, NS_BITMAP_IMAGE_FILE_TYPE_PNG, properties)
            else {
                continue;
            };
            if write_data_to_url(png, output_url.as_concrete_TypeRef()) {
                pages.push(output);
            }
        }

        release_if_present(document);
        release_if_present(pool);
        if pages.is_empty() {
            rasterization(
                NativePdfPageRasterizationStatus::Empty,
                pages,
                format!("PDFKit produced no OCR page images for {}", pdf.display()),
            )
        } else {
            NativePdfPageRasterization {
                pages,
                status: NativePdfPageRasterizationStatus::Available,
                reason: None,
            }
        }
    }
}

fn rasterization(
    status: NativePdfPageRasterizationStatus,
    pages: Vec<PathBuf>,
    reason: impl Into<String>,
) -> NativePdfPageRasterization {
    NativePdfPageRasterization {
        pages,
        status,
        reason: Some(reason.into()),
    }
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

unsafe fn init_pdf_document_with_url(
    document: *mut Object,
    url: core_foundation_sys::url::CFURLRef,
) -> *mut Object {
    let send: unsafe extern "C" fn(
        *mut Object,
        Sel,
        core_foundation_sys::url::CFURLRef,
    ) -> *mut Object = std::mem::transmute(objc_msgSend as *const ());
    send(document, Sel::register("initWithURL:"), url)
}

unsafe fn page_count(document: *mut Object) -> usize {
    let send: unsafe extern "C" fn(*mut Object, Sel) -> usize =
        std::mem::transmute(objc_msgSend as *const ());
    send(document, Sel::register("pageCount"))
}

unsafe fn page_at_index(document: *mut Object, index: usize) -> *mut Object {
    let send: unsafe extern "C" fn(*mut Object, Sel, usize) -> *mut Object =
        std::mem::transmute(objc_msgSend as *const ());
    send(document, Sel::register("pageAtIndex:"), index)
}

unsafe fn bounded_page_size(page: *mut Object, max_dimension_px: u32) -> NSSize {
    let send: unsafe extern "C" fn(*mut Object, Sel, i32) -> NSRect =
        std::mem::transmute(objc_msgSend as *const ());
    let bounds = send(
        page,
        Sel::register("boundsForBox:"),
        PDF_DISPLAY_BOX_MEDIA_BOX,
    );
    let width = bounds.size.width.max(1.0);
    let height = bounds.size.height.max(1.0);
    let max_dimension = f64::from(max_dimension_px);
    let scale = (max_dimension / width.max(height)).min(1.0);
    NSSize {
        width: (width * scale).max(1.0),
        height: (height * scale).max(1.0),
    }
}

unsafe fn thumbnail_for_page(page: *mut Object, size: NSSize) -> *mut Object {
    let send: unsafe extern "C" fn(*mut Object, Sel, NSSize, i32) -> *mut Object =
        std::mem::transmute(objc_msgSend as *const ());
    send(
        page,
        Sel::register("thumbnailOfSize:forBox:"),
        size,
        PDF_DISPLAY_BOX_MEDIA_BOX,
    )
}

unsafe fn image_tiff_representation(image: *mut Object) -> Option<*mut Object> {
    let send: unsafe extern "C" fn(*mut Object, Sel) -> *mut Object =
        std::mem::transmute(objc_msgSend as *const ());
    let data = send(image, Sel::register("TIFFRepresentation"));
    (!data.is_null()).then_some(data)
}

unsafe fn bitmap_rep_with_data(class: &Class, data: *mut Object) -> *mut Object {
    let send: unsafe extern "C" fn(&Class, Sel, *mut Object) -> *mut Object =
        std::mem::transmute(objc_msgSend as *const ());
    send(class, Sel::register("imageRepWithData:"), data)
}

unsafe fn bitmap_representation(
    bitmap: *mut Object,
    file_type: usize,
    properties: *mut Object,
) -> Option<*mut Object> {
    let send: unsafe extern "C" fn(*mut Object, Sel, usize, *mut Object) -> *mut Object =
        std::mem::transmute(objc_msgSend as *const ());
    let data = send(
        bitmap,
        Sel::register("representationUsingType:properties:"),
        file_type,
        properties,
    );
    (!data.is_null()).then_some(data)
}

unsafe fn write_data_to_url(data: *mut Object, url: core_foundation_sys::url::CFURLRef) -> bool {
    let send: unsafe extern "C" fn(*mut Object, Sel, core_foundation_sys::url::CFURLRef, i8) -> i8 =
        std::mem::transmute(objc_msgSend as *const ());
    send(data, Sel::register("writeToURL:atomically:"), url, 1) != 0
}

unsafe fn release_if_present(object: *mut Object) {
    if object.is_null() {
        return;
    }
    let send: unsafe extern "C" fn(*mut Object, Sel) =
        std::mem::transmute(objc_msgSend as *const ());
    send(object, Sel::register("release"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("gfm-native-pdf-{label}-{}", std::process::id()))
    }

    #[test]
    fn pdf_rasterization_reports_missing_without_loading_document() {
        let pdf = unique_path("missing").with_extension("pdf");
        let output = unique_path("missing-output");

        let report = rasterize_pdf_pages_for_ocr(&pdf, &output, 1, 512);

        assert_eq!(report.status, NativePdfPageRasterizationStatus::Missing);
        assert!(report.pages.is_empty());
        assert!(report
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("PDF OCR rasterization input does not exist")));
    }

    #[test]
    fn pdf_rasterization_rejects_zero_limits() {
        let pdf = unique_path("zero").with_extension("pdf");
        fs::write(&pdf, b"%PDF-1.4\n").unwrap();
        let output = unique_path("zero-output");

        let no_pages = rasterize_pdf_pages_for_ocr(&pdf, &output, 0, 512);
        let no_dimension = rasterize_pdf_pages_for_ocr(&pdf, &output, 1, 0);

        assert_eq!(
            no_pages.status,
            NativePdfPageRasterizationStatus::Unsupported
        );
        assert_eq!(
            no_dimension.status,
            NativePdfPageRasterizationStatus::Unsupported
        );
        let _ = fs::remove_file(pdf);
    }

    #[test]
    fn pdf_rasterization_writes_bounded_page_png_for_valid_pdf() {
        let root = unique_path("render");
        let pdf = root.with_extension("pdf");
        let output = root.with_extension("pages");
        fs::write(&pdf, single_page_pdf()).unwrap();

        let report = rasterize_pdf_pages_for_ocr(&pdf, &output, 1, 256);

        assert_eq!(report.status, NativePdfPageRasterizationStatus::Available);
        assert_eq!(report.pages.len(), 1);
        let png = fs::read(&report.pages[0]).unwrap();
        assert!(
            png.starts_with(b"\x89PNG\r\n\x1a\n"),
            "PDFKit output should be PNG"
        );
        let _ = fs::remove_file(pdf);
        let _ = fs::remove_dir_all(output);
    }

    fn single_page_pdf() -> Vec<u8> {
        let mut pdf = Vec::new();
        pdf.extend_from_slice(b"%PDF-1.4\n");
        let mut offsets = vec![0_u64];
        push_pdf_object(
            &mut pdf,
            &mut offsets,
            b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        );
        push_pdf_object(
            &mut pdf,
            &mut offsets,
            b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n",
        );
        push_pdf_object(
            &mut pdf,
            &mut offsets,
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents 4 0 R >>\nendobj\n",
        );
        push_pdf_object(
            &mut pdf,
            &mut offsets,
            b"4 0 obj\n<< /Length 0 >>\nstream\n\nendstream\nendobj\n",
        );
        let xref = pdf.len();
        pdf.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
        for offset in offsets.iter().skip(1) {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        pdf
    }

    fn push_pdf_object(pdf: &mut Vec<u8>, offsets: &mut Vec<u64>, object: &[u8]) {
        offsets.push(pdf.len() as u64);
        pdf.extend_from_slice(object);
    }
}
