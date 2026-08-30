mod archive;
mod cache;
mod kind;
mod ooxml;
mod pdf;
mod policy;
mod quarantine;
mod report;
mod rich;
mod status;
mod structured;

use archive::extract_archive_metadata_checked;
pub use cache::{
    CachedExtractionReport, CachedExtractor, ExtractionCache, ExtractionCacheKey,
    ExtractionCacheStatus, ExtractionContentSignature,
};
use gfm_types::{FileKind, FileRecord, GfmError, Result, SearchSnippet, SnippetHighlight};
pub use kind::extractor_version_for_path;
use kind::{archive_kind, extraction_format, office_kind, path_is_pdf, rich_kind, structured_kind};
use ooxml::extract_ooxml_checked;
use pdf::extract_pdf_checked;
pub use policy::{
    ExtractionBatteryState, ExtractionBudgetProfile, ExtractionPolicy, ExtractionThermalState,
    ExtractionUserActivity, ExtractionVolumeClass,
};
pub use quarantine::{
    ExtractionQuarantine, QuarantineDecision, QuarantineEntry, QuarantineFailureKind,
    EXTRACTION_QUARANTINE_SCHEMA_VERSION,
};
pub use report::{
    ContentDocument, ExtractionFingerprint, ExtractionFormat, ExtractionReport, ExtractionStatus,
};
use rich::extract_rich_checked;
use status::{
    archive_report_status, document_status, ooxml_report_status, pdf_report_status,
    structured_report_status,
};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use structured::extract_structured_checked;

pub const TEXT_EXTRACTOR_VERSION: u32 = 4;
pub const PDF_EXTRACTOR_VERSION: u32 = 3;
pub const OFFICE_EXTRACTOR_VERSION: u32 = 3;
pub const RICH_EXTRACTOR_VERSION: u32 = 6;
pub const ARCHIVE_EXTRACTOR_VERSION: u32 = 5;
pub const STRUCTURED_EXTRACTOR_VERSION: u32 = 3;
pub const UNSUPPORTED_EXTRACTOR_VERSION: u32 = 1;
pub const EXTRACTOR_VERSION: u32 = ARCHIVE_EXTRACTOR_VERSION;
const EXTRACTION_READ_CHUNK_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone)]
pub struct Extractor {
    policy: ExtractionPolicy,
}

impl Extractor {
    pub fn new(policy: ExtractionPolicy) -> Self {
        Self { policy }
    }

    pub fn with_budget_profile(profile: ExtractionBudgetProfile) -> Self {
        Self::new(profile.policy())
    }

    pub fn extract_record(&self, record: &FileRecord) -> Result<Option<ContentDocument>> {
        self.extract_record_checked(record, || Ok(()))
    }

    pub fn extract_record_checked(
        &self,
        record: &FileRecord,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Option<ContentDocument>> {
        check_control()?;
        if record.kind != FileKind::File {
            return Ok(None);
        }
        let max_bytes = if path_is_pdf(&record.path) {
            self.policy.max_pdf_bytes
        } else if office_kind(&record.path).is_some() {
            self.policy.max_office_bytes
        } else if archive_kind(&record.path).is_some() {
            self.policy.max_archive_bytes
        } else {
            self.policy.max_bytes
        };
        if record.len > max_bytes {
            return Ok(None);
        }
        if !self.accepts_path(&record.path) {
            return Ok(None);
        }
        self.extract_path_checked(&record.path, check_control)
    }

    pub fn snippet_for_record(
        &self,
        record: &FileRecord,
        terms: &[String],
        phrases: &[String],
        context_bytes: usize,
    ) -> Result<Option<SearchSnippet>> {
        self.snippet_for_record_checked(record, terms, phrases, context_bytes, || Ok(()))
    }

    pub fn snippet_for_record_checked(
        &self,
        record: &FileRecord,
        terms: &[String],
        phrases: &[String],
        context_bytes: usize,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Option<SearchSnippet>> {
        let Some(document) = self.extract_record_checked(record, &mut check_control)? else {
            return Ok(None);
        };
        check_control()?;
        build_snippet_checked(
            &document.text,
            terms,
            phrases,
            context_bytes.max(1),
            &mut check_control,
        )
    }

    pub fn extract_path(&self, path: impl AsRef<Path>) -> Result<Option<ContentDocument>> {
        self.extract_path_checked(path, || Ok(()))
    }

    pub fn extract_path_checked(
        &self,
        path: impl AsRef<Path>,
        check_control: impl FnMut() -> Result<()>,
    ) -> Result<Option<ContentDocument>> {
        Ok(self
            .extract_path_report_checked(path, check_control)?
            .document)
    }

    pub fn extract_path_report(&self, path: impl AsRef<Path>) -> Result<ExtractionReport> {
        self.extract_path_report_checked(path, || Ok(()))
    }

    pub fn extract_path_report_checked(
        &self,
        path: impl AsRef<Path>,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<ExtractionReport> {
        let path = path.as_ref();
        check_control()?;
        if !self.accepts_path(path) {
            return Ok(report_without_metadata(
                path,
                ExtractionFormat::Unsupported,
                ExtractionStatus::Skipped("unsupported-extension"),
            ));
        }
        check_control()?;
        let metadata = std::fs::metadata(path).map_err(|err| GfmError::io(path, err))?;
        check_control()?;
        let extractor_version = extractor_version_for_path(path);
        let fingerprint = ExtractionFingerprint::from_metadata(&metadata, extractor_version);
        let office = office_kind(path);
        let rich = rich_kind(path);
        let archive = archive_kind(path);
        let structured = structured_kind(path);
        let is_pdf = path_is_pdf(path);
        let format = extraction_format(is_pdf, office, archive, rich, structured);
        let max_bytes = if is_pdf {
            self.policy.max_pdf_bytes
        } else if office.is_some() {
            self.policy.max_office_bytes
        } else if archive.is_some() {
            self.policy.max_archive_bytes
        } else {
            self.policy.max_bytes
        };
        if metadata.len() > max_bytes {
            return Ok(ExtractionReport {
                path: path.to_path_buf(),
                format,
                status: ExtractionStatus::Skipped("too-large"),
                fingerprint,
                document: None,
            });
        }

        let bytes = read_limited_file_checked(path, max_bytes, metadata.len(), &mut check_control)?;
        check_control()?;

        if is_pdf {
            let (status, document) = extract_pdf_checked(&bytes, &self.policy, &mut check_control)?;
            return Ok(ExtractionReport {
                path: path.to_path_buf(),
                format,
                status: pdf_report_status(status),
                fingerprint,
                document,
            });
        }

        if let Some(kind) = office {
            let (status, document) =
                extract_ooxml_checked(&bytes, kind, &self.policy, &mut check_control)?;
            return Ok(ExtractionReport {
                path: path.to_path_buf(),
                format,
                status: ooxml_report_status(status),
                fingerprint,
                document,
            });
        }

        if let Some(kind) = archive {
            let (status, document) =
                extract_archive_metadata_checked(&bytes, kind, &self.policy, &mut check_control)?;
            return Ok(ExtractionReport {
                path: path.to_path_buf(),
                format,
                status: archive_report_status(status),
                fingerprint,
                document,
            });
        }

        if let Some(kind) = rich {
            let document = extract_rich_checked(
                &bytes,
                kind,
                self.policy.max_rich_text_bytes,
                &mut check_control,
            )?;
            return Ok(ExtractionReport {
                path: path.to_path_buf(),
                format,
                status: document_status(document.as_ref()),
                fingerprint,
                document,
            });
        }

        if let Some(kind) = structured {
            let (status, document) =
                extract_structured_checked(&bytes, kind, &self.policy, &mut check_control)?;
            return Ok(ExtractionReport {
                path: path.to_path_buf(),
                format,
                status: structured_report_status(status),
                fingerprint,
                document,
            });
        }

        if is_binary(&bytes) {
            return Ok(ExtractionReport {
                path: path.to_path_buf(),
                format,
                status: ExtractionStatus::Skipped("binary"),
                fingerprint,
                document: None,
            });
        }

        let bytes_read = bytes.len();
        let Ok(text) = String::from_utf8(bytes) else {
            return Ok(ExtractionReport {
                path: path.to_path_buf(),
                format,
                status: ExtractionStatus::Skipped("non-utf8"),
                fingerprint,
                document: None,
            });
        };
        let normalized = normalize_text_checked(&text, &mut check_control)?;
        let text =
            truncate_text_checked(&normalized, self.policy.max_text_bytes, &mut check_control)?;

        let document = ContentDocument { bytes_read, text };
        Ok(ExtractionReport {
            path: path.to_path_buf(),
            format,
            status: ExtractionStatus::Extracted,
            fingerprint,
            document: Some(document),
        })
    }

    fn accepts_path(&self, path: &Path) -> bool {
        path_is_pdf(path)
            || office_kind(path).is_some()
            || archive_kind(path).is_some()
            || rich_kind(path).is_some()
            || structured_kind(path).is_some()
            || path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| self.policy.extensions.contains(&extension.to_lowercase()))
                .unwrap_or(false)
    }
}

fn read_limited_file_checked(
    path: &Path,
    max_bytes: u64,
    len: u64,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<u8>> {
    check_control()?;
    let file = File::open(path).map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    let mut reader = file.take(max_bytes);
    let mut bytes = Vec::with_capacity(len.min(max_bytes) as usize);
    let mut buffer = [0_u8; EXTRACTION_READ_CHUNK_BYTES];
    loop {
        check_control()?;
        let read = reader
            .read(&mut buffer)
            .map_err(|err| GfmError::io(path, err))?;
        check_control()?;
        if read == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
}

fn report_without_metadata(
    path: &Path,
    format: ExtractionFormat,
    status: ExtractionStatus,
) -> ExtractionReport {
    ExtractionReport {
        path: path.to_path_buf(),
        format,
        status,
        fingerprint: ExtractionFingerprint {
            extractor_version: extractor_version_for_path(path),
            len: 0,
            modified_ns: None,
        },
        document: None,
    }
}

fn build_snippet_checked(
    text: &str,
    terms: &[String],
    phrases: &[String],
    context_bytes: usize,
    check_control: &mut dyn FnMut() -> Result<()>,
) -> Result<Option<SearchSnippet>> {
    let normalized = lowercase_checked(text, check_control)?;
    let mut needles: Vec<_> = phrases
        .iter()
        .chain(terms.iter())
        .filter_map(|needle| {
            let needle = needle.trim().to_lowercase();
            (!needle.is_empty()).then_some(needle)
        })
        .collect();
    check_control()?;
    needles.sort_by_key(|needle| std::cmp::Reverse(needle.len()));

    let mut matched = None;
    for needle in &needles {
        check_control()?;
        if let Some(start) = normalized.find(needle) {
            matched = Some((start, start + needle.len()));
            break;
        }
    }
    let Some((match_start, match_end)) = matched else {
        return Ok(None);
    };
    check_control()?;
    let snippet_start = floor_char_boundary(text, match_start.saturating_sub(context_bytes));
    let snippet_end = ceil_char_boundary(text, (match_end + context_bytes).min(text.len()));
    let snippet_text = text[snippet_start..snippet_end].to_string();
    let highlight_start = match_start.saturating_sub(snippet_start);
    let highlight_end = match_end.saturating_sub(snippet_start);

    Ok(Some(SearchSnippet {
        text: snippet_text,
        highlights: vec![SnippetHighlight {
            start: highlight_start,
            end: highlight_end,
        }],
    }))
}

fn lowercase_checked(input: &str, check_control: &mut dyn FnMut() -> Result<()>) -> Result<String> {
    let mut output = String::with_capacity(input.len());
    for ch in input.chars() {
        check_control()?;
        output.extend(ch.to_lowercase());
    }
    Ok(output)
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

impl Default for Extractor {
    fn default() -> Self {
        Self::new(ExtractionPolicy::default())
    }
}

fn normalize_text_checked(
    input: &str,
    check_control: &mut dyn FnMut() -> Result<()>,
) -> Result<String> {
    let mut output = String::with_capacity(input.len());
    for ch in input.chars() {
        check_control()?;
        if ch.is_control() && ch != '\n' && ch != '\t' {
            output.push(' ');
        } else {
            output.push(ch);
        }
    }
    Ok(output)
}

fn truncate_text(text: &str, max_bytes: usize) -> String {
    let end = floor_char_boundary(text, max_bytes);
    text[..end].to_string()
}

fn truncate_text_checked(
    text: &str,
    max_bytes: usize,
    check_control: &mut dyn FnMut() -> Result<()>,
) -> Result<String> {
    check_control()?;
    Ok(truncate_text(text, max_bytes))
}

fn is_binary(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(4096)];
    if sample.is_empty() {
        return false;
    }
    if has_binary_signature(sample) {
        return true;
    }
    if sample.contains(&0) {
        return true;
    }

    let controls = sample
        .iter()
        .filter(|byte| matches!(**byte, 0x01..=0x08 | 0x0e..=0x1f | 0x7f))
        .count();
    sample.len() >= 32 && controls * 100 / sample.len() > 10
}

fn has_binary_signature(bytes: &[u8]) -> bool {
    const SIGNATURES: &[&[u8]] = &[
        b"\x7fELF",
        b"\x89PNG\r\n\x1a\n",
        b"\xff\xd8\xff",
        b"GIF87a",
        b"GIF89a",
        b"PK\x03\x04",
        b"PK\x05\x06",
        b"PK\x07\x08",
        b"\x1f\x8b",
        b"7z\xbc\xaf\x27\x1c",
        b"Rar!\x1a\x07\x00",
        b"Rar!\x1a\x07\x01\x00",
        b"\xca\xfe\xba\xbe",
        b"\xcf\xfa\xed\xfe",
        b"\xce\xfa\xed\xfe",
        b"\xfe\xed\xfa\xcf",
        b"\xfe\xed\xfa\xce",
        b"%PDF-",
        b"SQLite format 3\0",
        b"bplist00",
    ];
    SIGNATURES
        .iter()
        .any(|signature| bytes.starts_with(signature))
}

#[cfg(test)]
mod tests;
