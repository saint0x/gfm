use crate::archive::ArchiveExtractStatus;
use crate::ooxml::OoxmlExtractStatus;
use crate::pdf::PdfExtractStatus;
use crate::report::{ContentDocument, ExtractionStatus};
use crate::structured::StructuredExtractStatus;

pub(crate) fn pdf_report_status(status: PdfExtractStatus) -> ExtractionStatus {
    match status {
        PdfExtractStatus::Extracted => ExtractionStatus::Extracted,
        PdfExtractStatus::Unsupported => ExtractionStatus::Skipped("unsupported-pdf"),
        PdfExtractStatus::TooLarge => ExtractionStatus::Skipped("too-large"),
        PdfExtractStatus::TooManyPages => ExtractionStatus::Skipped("too-many-pages"),
        PdfExtractStatus::TooManyObjects => ExtractionStatus::Skipped("too-many-objects"),
        PdfExtractStatus::Encrypted => ExtractionStatus::Quarantined("encrypted-pdf"),
        PdfExtractStatus::Corrupt => ExtractionStatus::Quarantined("corrupt-pdf"),
    }
}

pub(crate) fn ooxml_report_status(status: OoxmlExtractStatus) -> ExtractionStatus {
    match status {
        OoxmlExtractStatus::Extracted => ExtractionStatus::Extracted,
        OoxmlExtractStatus::Unsupported => ExtractionStatus::Skipped("unsupported-office"),
        OoxmlExtractStatus::TooLarge => ExtractionStatus::Skipped("too-large"),
        OoxmlExtractStatus::TooManyEntries => ExtractionStatus::Skipped("too-many-entries"),
        OoxmlExtractStatus::EntryTooLarge => ExtractionStatus::Skipped("entry-too-large"),
        OoxmlExtractStatus::Corrupt => ExtractionStatus::Quarantined("corrupt-office"),
    }
}

pub(crate) fn archive_report_status(status: ArchiveExtractStatus) -> ExtractionStatus {
    match status {
        ArchiveExtractStatus::Extracted => ExtractionStatus::Extracted,
        ArchiveExtractStatus::Unsupported => ExtractionStatus::Skipped("unsupported-archive"),
        ArchiveExtractStatus::TooLarge => ExtractionStatus::Skipped("too-large"),
        ArchiveExtractStatus::TooManyEntries => ExtractionStatus::Skipped("too-many-entries"),
        ArchiveExtractStatus::Corrupt => ExtractionStatus::Quarantined("corrupt-archive"),
    }
}

pub(crate) fn structured_report_status(status: StructuredExtractStatus) -> ExtractionStatus {
    match status {
        StructuredExtractStatus::Extracted => ExtractionStatus::Extracted,
        StructuredExtractStatus::Unsupported => ExtractionStatus::Skipped("unsupported-structured"),
        StructuredExtractStatus::TooLarge => ExtractionStatus::Skipped("too-large"),
        StructuredExtractStatus::Corrupt => ExtractionStatus::Quarantined("corrupt-structured"),
    }
}

pub(crate) fn document_status(document: Option<&ContentDocument>) -> ExtractionStatus {
    if document.is_some() {
        ExtractionStatus::Extracted
    } else {
        ExtractionStatus::Skipped("no-text")
    }
}
