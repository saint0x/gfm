use crate::report::{ExtractionFingerprint, ExtractionFormat, ExtractionReport, ExtractionStatus};
use gfm_types::{FileKind, FileRecord};
use std::path::{Path, PathBuf};

pub const OCR_EXTRACTOR_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcrCandidateKind {
    ImageOnlyPdf,
    ScreenshotImage,
}

impl OcrCandidateKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ImageOnlyPdf => "image-only-pdf",
            Self::ScreenshotImage => "screenshot-image",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrCandidate {
    pub path: PathBuf,
    pub kind: OcrCandidateKind,
    pub fingerprint: ExtractionFingerprint,
}

impl OcrCandidate {
    pub fn as_tsv(&self) -> String {
        format!(
            "ocr-candidate\tpath={}\tkind={}\tversion={}\tlen={}",
            crate::report::escape_report_field(&self.path.to_string_lossy()),
            self.kind.as_str(),
            self.fingerprint.extractor_version,
            self.fingerprint.len
        )
    }
}

pub fn ocr_candidate_for_record(record: &FileRecord) -> Option<OcrCandidate> {
    if record.kind != FileKind::File || !path_is_screenshot_image(&record.path) {
        return None;
    }
    Some(OcrCandidate {
        path: record.path.clone(),
        kind: OcrCandidateKind::ScreenshotImage,
        fingerprint: record_ocr_fingerprint(record),
    })
}

pub fn ocr_candidate_for_extraction(report: &ExtractionReport) -> Option<OcrCandidate> {
    if report.format != ExtractionFormat::Pdf
        || report.status != ExtractionStatus::Skipped("image-only-pdf")
    {
        return None;
    }
    Some(OcrCandidate {
        path: report.path.clone(),
        kind: OcrCandidateKind::ImageOnlyPdf,
        fingerprint: ExtractionFingerprint {
            extractor_version: OCR_EXTRACTOR_VERSION,
            len: report.fingerprint.len,
            modified_ns: report.fingerprint.modified_ns,
        },
    })
}

fn record_ocr_fingerprint(record: &FileRecord) -> ExtractionFingerprint {
    ExtractionFingerprint {
        extractor_version: OCR_EXTRACTOR_VERSION,
        len: record.len,
        modified_ns: record
            .modified
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos()),
    }
}

fn path_is_screenshot_image(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
        return false;
    };
    if !matches!(
        extension.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "heic" | "heif" | "tif" | "tiff"
    ) {
        return false;
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    name.contains("screenshot") || name.contains("screen shot")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::ContentDocument;
    use gfm_types::{FileId, VolumeId};
    use std::path::PathBuf;
    use std::time::UNIX_EPOCH;

    #[test]
    fn plans_screenshot_image_ocr_from_record_metadata() {
        let record = FileRecord {
            id: FileId::new(VolumeId(1), 7),
            parent: None,
            path: PathBuf::from("/tmp/Screenshot 2026-08-24 at 6.59.43 PM.png"),
            name: "Screenshot 2026-08-24 at 6.59.43 PM.png".to_string(),
            kind: FileKind::File,
            len: 2048,
            mode: 0,
            owner: 0,
            group: 0,
            xattrs_digest: 0,
            created: None,
            modified: Some(UNIX_EPOCH + std::time::Duration::from_nanos(42)),
            changed: None,
            hidden: false,
            tags: Vec::new(),
            finder_comment: None,
        };

        let candidate = ocr_candidate_for_record(&record).unwrap();

        assert_eq!(candidate.kind, OcrCandidateKind::ScreenshotImage);
        assert_eq!(
            candidate.fingerprint.extractor_version,
            OCR_EXTRACTOR_VERSION
        );
        assert_eq!(candidate.fingerprint.len, 2048);
        assert_eq!(candidate.fingerprint.modified_ns, Some(42));
        assert!(candidate.as_tsv().contains("\tkind=screenshot-image\t"));
    }

    #[test]
    fn plans_image_only_pdf_ocr_from_extraction_report() {
        let report = ExtractionReport {
            path: PathBuf::from("/tmp/scan.pdf"),
            format: ExtractionFormat::Pdf,
            status: ExtractionStatus::Skipped("image-only-pdf"),
            fingerprint: ExtractionFingerprint {
                extractor_version: 4,
                len: 4096,
                modified_ns: Some(99),
            },
            document: None,
        };

        let candidate = ocr_candidate_for_extraction(&report).unwrap();

        assert_eq!(candidate.kind, OcrCandidateKind::ImageOnlyPdf);
        assert_eq!(
            candidate.fingerprint.extractor_version,
            OCR_EXTRACTOR_VERSION
        );
        assert_eq!(candidate.fingerprint.len, 4096);
        assert_eq!(candidate.fingerprint.modified_ns, Some(99));
    }

    #[test]
    fn does_not_plan_ocr_for_extracted_pdf_text() {
        let report = ExtractionReport {
            path: PathBuf::from("/tmp/text.pdf"),
            format: ExtractionFormat::Pdf,
            status: ExtractionStatus::Extracted,
            fingerprint: ExtractionFingerprint {
                extractor_version: 4,
                len: 1024,
                modified_ns: None,
            },
            document: Some(ContentDocument {
                bytes_read: 1024,
                text: "already indexed".to_string(),
            }),
        };

        assert!(ocr_candidate_for_extraction(&report).is_none());
    }
}
