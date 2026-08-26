use crate::archive::ArchiveKind;
use crate::ooxml::OoxmlKind;
use crate::policy::text_extension_is_known;
use crate::report::ExtractionFormat;
use crate::rich::RichKind;
use crate::structured::StructuredKind;
use crate::{
    ARCHIVE_EXTRACTOR_VERSION, OFFICE_EXTRACTOR_VERSION, PDF_EXTRACTOR_VERSION,
    RICH_EXTRACTOR_VERSION, STRUCTURED_EXTRACTOR_VERSION, TEXT_EXTRACTOR_VERSION,
    UNSUPPORTED_EXTRACTOR_VERSION,
};
use std::path::Path;

pub fn extractor_version_for_path(path: &Path) -> u32 {
    if path_is_pdf(path) {
        PDF_EXTRACTOR_VERSION
    } else if office_kind(path).is_some() {
        OFFICE_EXTRACTOR_VERSION
    } else if archive_kind(path).is_some() {
        ARCHIVE_EXTRACTOR_VERSION
    } else if rich_kind(path).is_some() {
        RICH_EXTRACTOR_VERSION
    } else if structured_kind(path).is_some() {
        STRUCTURED_EXTRACTOR_VERSION
    } else if path_is_known_text(path) {
        TEXT_EXTRACTOR_VERSION
    } else {
        UNSUPPORTED_EXTRACTOR_VERSION
    }
}

pub(crate) fn extraction_format(
    is_pdf: bool,
    office: Option<OoxmlKind>,
    archive: Option<ArchiveKind>,
    rich: Option<RichKind>,
    structured: Option<StructuredKind>,
) -> ExtractionFormat {
    if is_pdf {
        ExtractionFormat::Pdf
    } else if office.is_some() {
        ExtractionFormat::Office
    } else if archive.is_some() {
        ExtractionFormat::Archive
    } else if rich.is_some() {
        ExtractionFormat::Rich
    } else if structured.is_some() {
        ExtractionFormat::Structured
    } else {
        ExtractionFormat::Text
    }
}

pub(crate) fn structured_kind(path: &Path) -> Option<StructuredKind> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "json" => Some(StructuredKind::Json),
        "csv" => Some(StructuredKind::Csv),
        "plist" => Some(StructuredKind::Plist),
        _ => None,
    }
}

pub(crate) fn rich_kind(path: &Path) -> Option<RichKind> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "html" | "htm" => Some(RichKind::Html),
        "rtf" => Some(RichKind::Rtf),
        "eml" => Some(RichKind::Email),
        _ => None,
    }
}

pub(crate) fn archive_kind(path: &Path) -> Option<ArchiveKind> {
    let file_name = path.file_name()?.to_str()?.to_ascii_lowercase();
    if file_name.ends_with(".tar.gz") || file_name.ends_with(".tgz") {
        return Some(ArchiveKind::TarGz);
    }
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "tar" => Some(ArchiveKind::Tar),
        "zip" => Some(ArchiveKind::Zip),
        _ => None,
    }
}

pub(crate) fn office_kind(path: &Path) -> Option<OoxmlKind> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "docx" => Some(OoxmlKind::Docx),
        "xlsx" => Some(OoxmlKind::Xlsx),
        "pptx" => Some(OoxmlKind::Pptx),
        _ => None,
    }
}

pub(crate) fn path_is_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

fn path_is_known_text(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(text_extension_is_known)
}
