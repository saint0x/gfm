mod archive;
mod cache;
mod ooxml;
mod pdf;
mod quarantine;
mod rich;
mod structured;

use archive::{extract_archive_metadata, ArchiveExtractStatus, ArchiveKind};
pub use cache::{
    CachedExtractionReport, CachedExtractor, ExtractionCache, ExtractionCacheKey,
    ExtractionCacheStatus, ExtractionContentSignature,
};
use gfm_types::{FileKind, FileRecord, GfmError, Result, SearchSnippet, SnippetHighlight};
use ooxml::{extract_ooxml, OoxmlExtractStatus, OoxmlKind};
use pdf::{extract_pdf, PdfExtractStatus};
pub use quarantine::{
    ExtractionQuarantine, QuarantineDecision, QuarantineEntry, QuarantineFailureKind,
    EXTRACTION_QUARANTINE_SCHEMA_VERSION,
};
use rich::{extract_rich, RichKind};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use structured::{extract_structured, StructuredExtractStatus, StructuredKind};

pub const EXTRACTOR_VERSION: u32 = 3;

#[derive(Debug, Clone)]
pub struct ExtractionPolicy {
    pub max_bytes: u64,
    pub max_pdf_bytes: u64,
    pub max_pdf_pages: usize,
    pub max_pdf_objects: usize,
    pub max_pdf_stream_bytes: usize,
    pub max_office_bytes: u64,
    pub max_office_entries: usize,
    pub max_office_entry_bytes: u64,
    pub max_office_text_bytes: usize,
    pub max_archive_bytes: u64,
    pub max_archive_entries: usize,
    pub max_archive_text_bytes: usize,
    pub max_structured_text_bytes: usize,
    pub extensions: BTreeSet<String>,
}

impl ExtractionPolicy {
    fn scaled(&self, percent: u64) -> Self {
        let percent = percent.clamp(1, 100);
        Self {
            max_bytes: scale_u64(self.max_bytes, percent).max(16 * 1024),
            max_pdf_bytes: scale_u64(self.max_pdf_bytes, percent).max(256 * 1024),
            max_pdf_pages: scale_usize(self.max_pdf_pages, percent).max(1),
            max_pdf_objects: scale_usize(self.max_pdf_objects, percent).max(128),
            max_pdf_stream_bytes: scale_usize(self.max_pdf_stream_bytes, percent).max(128 * 1024),
            max_office_bytes: scale_u64(self.max_office_bytes, percent).max(512 * 1024),
            max_office_entries: scale_usize(self.max_office_entries, percent).max(64),
            max_office_entry_bytes: scale_u64(self.max_office_entry_bytes, percent).max(128 * 1024),
            max_office_text_bytes: scale_usize(self.max_office_text_bytes, percent).max(64 * 1024),
            max_archive_bytes: scale_u64(self.max_archive_bytes, percent).max(512 * 1024),
            max_archive_entries: scale_usize(self.max_archive_entries, percent).max(64),
            max_archive_text_bytes: scale_usize(self.max_archive_text_bytes, percent)
                .max(64 * 1024),
            max_structured_text_bytes: scale_usize(self.max_structured_text_bytes, percent)
                .max(64 * 1024),
            extensions: self.extensions.clone(),
        }
    }
}

impl Default for ExtractionPolicy {
    fn default() -> Self {
        Self {
            max_bytes: 2 * 1024 * 1024,
            max_pdf_bytes: 16 * 1024 * 1024,
            max_pdf_pages: 256,
            max_pdf_objects: 20_000,
            max_pdf_stream_bytes: 8 * 1024 * 1024,
            max_office_bytes: 32 * 1024 * 1024,
            max_office_entries: 10_000,
            max_office_entry_bytes: 8 * 1024 * 1024,
            max_office_text_bytes: 4 * 1024 * 1024,
            max_archive_bytes: 64 * 1024 * 1024,
            max_archive_entries: 20_000,
            max_archive_text_bytes: 2 * 1024 * 1024,
            max_structured_text_bytes: 4 * 1024 * 1024,
            extensions: text_extensions(),
        }
    }
}

const fn min_percent(left: u64, right: u64) -> u64 {
    if left < right {
        left
    } else {
        right
    }
}

fn scale_u64(value: u64, percent: u64) -> u64 {
    value.saturating_mul(percent).div_ceil(100)
}

fn scale_usize(value: usize, percent: u64) -> usize {
    let scaled = (value as u64).saturating_mul(percent).div_ceil(100);
    scaled.min(usize::MAX as u64) as usize
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionVolumeClass {
    Local,
    External,
    Network,
    Cloud,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionThermalState {
    Nominal,
    Fair,
    Serious,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionBatteryState {
    AcPower,
    Battery,
    LowPower,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionUserActivity {
    Idle,
    Active,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractionBudgetProfile {
    pub volume: ExtractionVolumeClass,
    pub thermal: ExtractionThermalState,
    pub battery: ExtractionBatteryState,
    pub user_activity: ExtractionUserActivity,
}

impl Default for ExtractionBudgetProfile {
    fn default() -> Self {
        Self {
            volume: ExtractionVolumeClass::Local,
            thermal: ExtractionThermalState::Nominal,
            battery: ExtractionBatteryState::AcPower,
            user_activity: ExtractionUserActivity::Idle,
        }
    }
}

impl ExtractionBudgetProfile {
    pub fn policy(self) -> ExtractionPolicy {
        self.policy_from(ExtractionPolicy::default())
    }

    pub fn policy_from(self, base: ExtractionPolicy) -> ExtractionPolicy {
        base.scaled(self.scale_percent())
    }

    pub const fn scale_percent(self) -> u64 {
        let mut percent = match self.volume {
            ExtractionVolumeClass::Local => 100,
            ExtractionVolumeClass::External => 80,
            ExtractionVolumeClass::Cloud => 60,
            ExtractionVolumeClass::Network => 50,
        };
        percent = min_percent(
            percent,
            match self.thermal {
                ExtractionThermalState::Nominal => 100,
                ExtractionThermalState::Fair => 80,
                ExtractionThermalState::Serious => 50,
                ExtractionThermalState::Critical => 25,
            },
        );
        percent = min_percent(
            percent,
            match self.battery {
                ExtractionBatteryState::AcPower => 100,
                ExtractionBatteryState::Battery => 80,
                ExtractionBatteryState::LowPower => 50,
            },
        );
        min_percent(
            percent,
            match self.user_activity {
                ExtractionUserActivity::Idle => 100,
                ExtractionUserActivity::Active => 60,
            },
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentDocument {
    pub bytes_read: usize,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionFormat {
    Text,
    Pdf,
    Office,
    Archive,
    Rich,
    Structured,
    Unsupported,
}

impl ExtractionFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Pdf => "pdf",
            Self::Office => "office",
            Self::Archive => "archive",
            Self::Rich => "rich",
            Self::Structured => "structured",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractionStatus {
    Extracted,
    Skipped(&'static str),
    Quarantined(&'static str),
}

impl ExtractionStatus {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Extracted => "extracted",
            Self::Skipped(_) => "skipped",
            Self::Quarantined(_) => "quarantined",
        }
    }

    pub const fn reason(&self) -> &'static str {
        match self {
            Self::Extracted => "ok",
            Self::Skipped(reason) | Self::Quarantined(reason) => reason,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionFingerprint {
    pub extractor_version: u32,
    pub len: u64,
    pub modified_ns: Option<u128>,
}

impl ExtractionFingerprint {
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        let modified_ns = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos());
        Self {
            extractor_version: EXTRACTOR_VERSION,
            len: metadata.len(),
            modified_ns,
        }
    }

    pub fn for_path(path: &Path) -> Result<Self> {
        let metadata = std::fs::metadata(path).map_err(|err| GfmError::io(path, err))?;
        Ok(Self::from_metadata(&metadata))
    }

    pub fn cache_key(&self, path: &Path) -> String {
        format!(
            "v{}:{}:{}:{}",
            self.extractor_version,
            path.display(),
            self.len,
            self.modified_ns
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string())
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionReport {
    pub path: PathBuf,
    pub format: ExtractionFormat,
    pub status: ExtractionStatus,
    pub fingerprint: ExtractionFingerprint,
    pub document: Option<ContentDocument>,
}

impl ExtractionReport {
    pub fn as_tsv(&self) -> String {
        format!(
            "extract\tpath={}\tformat={}\tstatus={}\treason={}\tversion={}\tbytes-read={}\ttext-bytes={}",
            self.path.display(),
            self.format.as_str(),
            self.status.as_str(),
            self.status.reason(),
            self.fingerprint.extractor_version,
            self.document
                .as_ref()
                .map(|document| document.bytes_read)
                .unwrap_or(0),
            self.document
                .as_ref()
                .map(|document| document.text.len())
                .unwrap_or(0)
        )
    }
}

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
        self.extract_path(&record.path)
    }

    pub fn snippet_for_record(
        &self,
        record: &FileRecord,
        terms: &[String],
        phrases: &[String],
        context_bytes: usize,
    ) -> Result<Option<SearchSnippet>> {
        let Some(document) = self.extract_record(record)? else {
            return Ok(None);
        };
        Ok(build_snippet(
            &document.text,
            terms,
            phrases,
            context_bytes.max(1),
        ))
    }

    pub fn extract_path(&self, path: impl AsRef<Path>) -> Result<Option<ContentDocument>> {
        Ok(self.extract_path_report(path)?.document)
    }

    pub fn extract_path_report(&self, path: impl AsRef<Path>) -> Result<ExtractionReport> {
        let path = path.as_ref();
        if !self.accepts_path(path) {
            return Ok(report_without_metadata(
                path,
                ExtractionFormat::Unsupported,
                ExtractionStatus::Skipped("unsupported-extension"),
            ));
        }
        let metadata = std::fs::metadata(path).map_err(|err| GfmError::io(path, err))?;
        let fingerprint = ExtractionFingerprint::from_metadata(&metadata);
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

        let file = File::open(path).map_err(|err| GfmError::io(path, err))?;
        let mut bytes = Vec::with_capacity(metadata.len().min(max_bytes) as usize);
        file.take(max_bytes)
            .read_to_end(&mut bytes)
            .map_err(|err| GfmError::io(path, err))?;

        if is_pdf {
            let (status, document) = extract_pdf(&bytes, &self.policy);
            return Ok(ExtractionReport {
                path: path.to_path_buf(),
                format,
                status: pdf_report_status(status),
                fingerprint,
                document,
            });
        }

        if let Some(kind) = office {
            let (status, document) = extract_ooxml(&bytes, kind, &self.policy);
            return Ok(ExtractionReport {
                path: path.to_path_buf(),
                format,
                status: ooxml_report_status(status),
                fingerprint,
                document,
            });
        }

        if let Some(kind) = archive {
            let (status, document) = extract_archive_metadata(&bytes, kind, &self.policy);
            return Ok(ExtractionReport {
                path: path.to_path_buf(),
                format,
                status: archive_report_status(status),
                fingerprint,
                document,
            });
        }

        if let Some(kind) = rich {
            let document = extract_rich(&bytes, kind);
            return Ok(ExtractionReport {
                path: path.to_path_buf(),
                format,
                status: document_status(document.as_ref()),
                fingerprint,
                document,
            });
        }

        if let Some(kind) = structured {
            let (status, document) = extract_structured(&bytes, kind, &self.policy);
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

        let Ok(text) = String::from_utf8(bytes) else {
            return Ok(ExtractionReport {
                path: path.to_path_buf(),
                format,
                status: ExtractionStatus::Skipped("non-utf8"),
                fingerprint,
                document: None,
            });
        };
        let text = normalize_text(&text);

        let document = ContentDocument {
            bytes_read: text.len(),
            text,
        };
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

fn extraction_format(
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

fn pdf_report_status(status: PdfExtractStatus) -> ExtractionStatus {
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

fn ooxml_report_status(status: OoxmlExtractStatus) -> ExtractionStatus {
    match status {
        OoxmlExtractStatus::Extracted => ExtractionStatus::Extracted,
        OoxmlExtractStatus::Unsupported => ExtractionStatus::Skipped("unsupported-office"),
        OoxmlExtractStatus::TooLarge => ExtractionStatus::Skipped("too-large"),
        OoxmlExtractStatus::TooManyEntries => ExtractionStatus::Skipped("too-many-entries"),
        OoxmlExtractStatus::EntryTooLarge => ExtractionStatus::Skipped("entry-too-large"),
        OoxmlExtractStatus::Corrupt => ExtractionStatus::Quarantined("corrupt-office"),
    }
}

fn archive_report_status(status: ArchiveExtractStatus) -> ExtractionStatus {
    match status {
        ArchiveExtractStatus::Extracted => ExtractionStatus::Extracted,
        ArchiveExtractStatus::Unsupported => ExtractionStatus::Skipped("unsupported-archive"),
        ArchiveExtractStatus::TooLarge => ExtractionStatus::Skipped("too-large"),
        ArchiveExtractStatus::TooManyEntries => ExtractionStatus::Skipped("too-many-entries"),
        ArchiveExtractStatus::Corrupt => ExtractionStatus::Quarantined("corrupt-archive"),
    }
}

fn structured_report_status(status: StructuredExtractStatus) -> ExtractionStatus {
    match status {
        StructuredExtractStatus::Extracted => ExtractionStatus::Extracted,
        StructuredExtractStatus::Unsupported => ExtractionStatus::Skipped("unsupported-structured"),
        StructuredExtractStatus::TooLarge => ExtractionStatus::Skipped("too-large"),
        StructuredExtractStatus::Corrupt => ExtractionStatus::Quarantined("corrupt-structured"),
    }
}

fn document_status(document: Option<&ContentDocument>) -> ExtractionStatus {
    if document.is_some() {
        ExtractionStatus::Extracted
    } else {
        ExtractionStatus::Skipped("no-text")
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
            extractor_version: EXTRACTOR_VERSION,
            len: 0,
            modified_ns: None,
        },
        document: None,
    }
}

fn structured_kind(path: &Path) -> Option<StructuredKind> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "json" => Some(StructuredKind::Json),
        "csv" => Some(StructuredKind::Csv),
        "plist" => Some(StructuredKind::Plist),
        _ => None,
    }
}

fn rich_kind(path: &Path) -> Option<RichKind> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "html" | "htm" => Some(RichKind::Html),
        "rtf" => Some(RichKind::Rtf),
        "eml" => Some(RichKind::Email),
        _ => None,
    }
}

fn archive_kind(path: &Path) -> Option<ArchiveKind> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "zip" => Some(ArchiveKind::Zip),
        _ => None,
    }
}

fn office_kind(path: &Path) -> Option<OoxmlKind> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "docx" => Some(OoxmlKind::Docx),
        "xlsx" => Some(OoxmlKind::Xlsx),
        "pptx" => Some(OoxmlKind::Pptx),
        _ => None,
    }
}

fn path_is_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

fn build_snippet(
    text: &str,
    terms: &[String],
    phrases: &[String],
    context_bytes: usize,
) -> Option<SearchSnippet> {
    let normalized = text.to_lowercase();
    let mut needles: Vec<_> = phrases
        .iter()
        .chain(terms.iter())
        .filter_map(|needle| {
            let needle = needle.trim().to_lowercase();
            (!needle.is_empty()).then_some(needle)
        })
        .collect();
    needles.sort_by_key(|needle| std::cmp::Reverse(needle.len()));

    let (match_start, match_end) = needles.iter().find_map(|needle| {
        normalized
            .find(needle)
            .map(|start| (start, start + needle.len()))
    })?;
    let snippet_start = floor_char_boundary(text, match_start.saturating_sub(context_bytes));
    let snippet_end = ceil_char_boundary(text, (match_end + context_bytes).min(text.len()));
    let snippet_text = text[snippet_start..snippet_end].to_string();
    let highlight_start = match_start.saturating_sub(snippet_start);
    let highlight_end = match_end.saturating_sub(snippet_start);

    Some(SearchSnippet {
        text: snippet_text,
        highlights: vec![SnippetHighlight {
            start: highlight_start,
            end: highlight_end,
        }],
    })
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

fn normalize_text(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_control() && ch != '\n' && ch != '\t' {
                ' '
            } else {
                ch
            }
        })
        .collect()
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

fn text_extensions() -> BTreeSet<String> {
    [
        "bash", "c", "cc", "conf", "cpp", "css", "csv", "go", "h", "hpp", "html", "java", "js",
        "json", "jsx", "log", "md", "mjs", "plist", "py", "rb", "rs", "sh", "sql", "swift", "toml",
        "ts", "tsx", "txt", "xml", "yaml", "yml", "zsh",
    ]
    .into_iter()
    .map(ToOwned::to_owned)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfm_types::{FileId, VolumeId};
    use std::fs;
    use std::io::{Cursor, Write};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};
    use zip::write::SimpleFileOptions;

    #[test]
    fn extracts_utf8_text_with_byte_budget() {
        let root = unique_temp_dir("gfm-content");
        let path = root.join("note.md");
        fs::write(&path, "hello content index").unwrap();
        let record = FileRecord {
            id: FileId::new(VolumeId(1), 1),
            parent: None,
            path: path.clone(),
            name: "note.md".to_string(),
            kind: FileKind::File,
            len: 19,
            mode: 0,
            owner: 0,
            group: 0,
            xattrs_digest: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: Vec::new(),
            finder_comment: None,
        };

        let doc = Extractor::default()
            .extract_record(&record)
            .unwrap()
            .unwrap();

        assert_eq!(doc.text, "hello content index");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extraction_budget_profile_scales_by_volume_and_host_pressure() {
        let profile = ExtractionBudgetProfile {
            volume: ExtractionVolumeClass::Network,
            thermal: ExtractionThermalState::Serious,
            battery: ExtractionBatteryState::LowPower,
            user_activity: ExtractionUserActivity::Active,
        };

        let policy = profile.policy();

        assert_eq!(profile.scale_percent(), 50);
        assert_eq!(policy.max_bytes, 1024 * 1024);
        assert_eq!(policy.max_pdf_bytes, 8 * 1024 * 1024);
        assert_eq!(policy.max_office_entries, 5_000);
    }

    #[test]
    fn pressure_budget_skips_large_text_before_reading_content() {
        let root = unique_temp_dir("gfm-content-pressure-budget");
        let path = root.join("large.txt");
        fs::write(&path, "x".repeat(1024 * 1024 + 1)).unwrap();
        let extractor = Extractor::with_budget_profile(ExtractionBudgetProfile {
            volume: ExtractionVolumeClass::Network,
            thermal: ExtractionThermalState::Serious,
            battery: ExtractionBatteryState::LowPower,
            user_activity: ExtractionUserActivity::Active,
        });

        let report = extractor.extract_path_report(&path).unwrap();

        assert_eq!(report.status, ExtractionStatus::Skipped("too-large"));
        assert!(report.document.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn skips_binary_files() {
        let root = unique_temp_dir("gfm-content-binary");
        let path = root.join("binary.txt");
        fs::write(&path, [0, 159, 146, 150]).unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap();

        assert!(doc.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn skips_known_binary_signatures_even_with_text_extension() {
        let root = unique_temp_dir("gfm-content-binary-signature");
        let path = root.join("image.txt");
        fs::write(&path, b"\x89PNG\r\n\x1a\nsuperneedle in binary payload").unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap();

        assert!(doc.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn skips_high_control_byte_payloads() {
        let root = unique_temp_dir("gfm-content-control-bytes");
        let path = root.join("controls.log");
        let mut bytes = b"prefix readable ".to_vec();
        bytes.extend([1, 2, 3, 4, 5, 6, 7, 8, 14, 15, 16, 17, 18, 19, 20, 21]);
        bytes.extend(b" suffix");
        fs::write(&path, bytes).unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap();

        assert!(doc.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn accepts_multibyte_utf8_text() {
        let root = unique_temp_dir("gfm-content-utf8");
        let path = root.join("note.txt");
        fs::write(&path, "cafe naive resume 東京").unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap().unwrap();

        assert!(doc.text.contains("東京"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_bounded_snippet_with_highlight() {
        let root = unique_temp_dir("gfm-content-snippet");
        let path = root.join("note.md");
        fs::write(
            &path,
            "before before before exact snippet marker after after after",
        )
        .unwrap();
        let record = FileRecord {
            id: FileId::new(VolumeId(1), 1),
            parent: None,
            path: path.clone(),
            name: "note.md".to_string(),
            kind: FileKind::File,
            len: 57,
            mode: 0,
            owner: 0,
            group: 0,
            xattrs_digest: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: Vec::new(),
            finder_comment: None,
        };

        let snippet = Extractor::default()
            .snippet_for_record(&record, &[], &["exact snippet".to_string()], 8)
            .unwrap()
            .unwrap();

        assert!(snippet.text.contains("exact snippet"));
        assert!(snippet.text.len() < 57);
        assert_eq!(
            &snippet.text[snippet.highlights[0].start..snippet.highlights[0].end],
            "exact snippet"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_uncompressed_pdf_text() {
        let root = unique_temp_dir("gfm-content-pdf");
        let path = root.join("brief.pdf");
        fs::write(&path, minimal_pdf("pdfneedle inside document")).unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap().unwrap();

        assert!(doc.text.contains("pdfneedle inside document"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn applies_pdf_byte_budget_to_records() {
        let root = unique_temp_dir("gfm-content-pdf-budget");
        let path = root.join("large.pdf");
        fs::write(&path, minimal_pdf("large pdf text")).unwrap();
        let record = FileRecord {
            id: FileId::new(VolumeId(1), 1),
            parent: None,
            path: path.clone(),
            name: "large.pdf".to_string(),
            kind: FileKind::File,
            len: fs::metadata(&path).unwrap().len(),
            mode: 0,
            owner: 0,
            group: 0,
            xattrs_digest: 0,
            created: None,
            modified: None,
            changed: None,
            hidden: false,
            tags: Vec::new(),
            finder_comment: None,
        };
        let extractor = Extractor::new(ExtractionPolicy {
            max_pdf_bytes: 12,
            ..ExtractionPolicy::default()
        });

        let doc = extractor.extract_record(&record).unwrap();

        assert!(doc.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn skips_pdf_when_page_budget_is_exceeded() {
        let root = unique_temp_dir("gfm-content-pdf-pages");
        let path = root.join("many.pdf");
        fs::write(&path, multi_page_pdf(4)).unwrap();
        let extractor = Extractor::new(ExtractionPolicy {
            max_pdf_pages: 3,
            ..ExtractionPolicy::default()
        });

        let doc = extractor.extract_path(&path).unwrap();

        assert!(doc.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_versioned_pdf_extraction_fingerprints() {
        let root = unique_temp_dir("gfm-content-pdf-report");
        let path = root.join("brief.pdf");
        fs::write(&path, minimal_pdf("versioned pdfneedle")).unwrap();

        let report = Extractor::default().extract_path_report(&path).unwrap();

        assert_eq!(report.format, ExtractionFormat::Pdf);
        assert_eq!(report.status, ExtractionStatus::Extracted);
        assert_eq!(report.fingerprint.extractor_version, EXTRACTOR_VERSION);
        assert!(report
            .fingerprint
            .cache_key(&path)
            .starts_with(&format!("v{EXTRACTOR_VERSION}:")));
        assert!(report.as_tsv().contains("\tstatus=extracted\t"));
        assert!(report
            .document
            .unwrap()
            .text
            .contains("versioned pdfneedle"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn quarantines_repeated_corrupt_pdf_failures_by_content_fingerprint() {
        let root = unique_temp_dir("gfm-content-pdf-quarantine");
        let path = root.join("corrupt.pdf");
        fs::write(
            &path,
            b"%PDF-1.4
1 0 obj
<< /Type /Page /Contents 2 0 R >>
endobj
2 0 obj
<< /Length 12 /Filter /FlateDecode >>
stream
not-valid-zlib
endstream
endobj",
        )
        .unwrap();
        let extractor = Extractor::default();
        let mut quarantine = ExtractionQuarantine::new(2);

        let first = extractor.extract_path_report(&path).unwrap();
        assert_eq!(first.status, ExtractionStatus::Quarantined("corrupt-pdf"));
        assert_eq!(quarantine.record_report(&first), QuarantineDecision::Allow);
        let second = extractor.extract_path_report(&path).unwrap();
        let decision = quarantine.record_report(&second);

        assert!(matches!(decision, QuarantineDecision::Quarantined(_)));
        assert!(matches!(
            quarantine.before_extract(&path, &second.fingerprint),
            QuarantineDecision::Quarantined(_)
        ));
        assert!(decision.as_tsv().contains("\treason=corrupt-pdf\t"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_docx_text() {
        let root = unique_temp_dir("gfm-content-docx");
        let path = root.join("brief.docx");
        fs::write(
            &path,
            ooxml_package(&[(
                "word/document.xml",
                "<w:document><w:body><w:p><w:r><w:t>docxneedle proposal</w:t></w:r></w:p></w:body></w:document>",
            )]),
        )
        .unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap().unwrap();

        assert_eq!(doc.text, "docxneedle proposal");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_xlsx_text() {
        let root = unique_temp_dir("gfm-content-xlsx");
        let path = root.join("numbers.xlsx");
        fs::write(
            &path,
            ooxml_package(&[(
                "xl/sharedStrings.xml",
                "<sst><si><t>sheetneedle</t></si><si><t>Revenue &amp; Margin</t></si></sst>",
            )]),
        )
        .unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap().unwrap();

        assert_eq!(doc.text, "sheetneedle Revenue & Margin");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_pptx_text() {
        let root = unique_temp_dir("gfm-content-pptx");
        let path = root.join("deck.pptx");
        fs::write(
            &path,
            ooxml_package(&[(
                "ppt/slides/slide1.xml",
                "<p:sld><p:cSld><a:t>slideneedle launch plan</a:t></p:cSld></p:sld>",
            )]),
        )
        .unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap().unwrap();

        assert_eq!(doc.text, "slideneedle launch plan");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_html_visible_text() {
        let root = unique_temp_dir("gfm-content-html");
        let path = root.join("page.html");
        fs::write(
            &path,
            "<html><body><h1>Visible &amp; searchable</h1><script>hiddenneedle</script><p>htmlneedle</p></body></html>",
        )
        .unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap().unwrap();

        assert_eq!(doc.text, "Visible & searchable htmlneedle");
        assert!(!doc.text.contains("hiddenneedle"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_rtf_text() {
        let root = unique_temp_dir("gfm-content-rtf");
        let path = root.join("note.rtf");
        fs::write(&path, br"{\rtf1\ansi rtfneedle\par rich text}").unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap().unwrap();

        assert_eq!(doc.text, "rtfneedle rich text");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_email_text() {
        let root = unique_temp_dir("gfm-content-email");
        let path = root.join("message.eml");
        fs::write(
            &path,
            b"From: Ada <ada@example.com>\r\nTo: Team\r\nSubject: Email Needle\r\n\r\nBody has emailneedle=20text",
        )
        .unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap().unwrap();

        assert!(doc.text.contains("Email Needle"));
        assert!(doc.text.contains("emailneedle text"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_zip_archive_metadata() {
        let root = unique_temp_dir("gfm-content-zip");
        let path = root.join("bundle.zip");
        fs::write(&path, zip_package(&[("docs/zipneedle.txt", "payload")])).unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap().unwrap();

        assert!(doc.text.contains("docs/zipneedle.txt"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_json_structure() {
        let root = unique_temp_dir("gfm-content-json");
        let path = root.join("data.json");
        fs::write(
            &path,
            br#"{"client":"Aperture","items":[{"name":"jsonneedle","count":3}]}"#,
        )
        .unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap().unwrap();

        assert!(doc.text.contains("client"));
        assert!(doc.text.contains("Aperture"));
        assert!(doc.text.contains("jsonneedle"));
        assert!(doc.text.contains("3"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_csv_cells() {
        let root = unique_temp_dir("gfm-content-csv");
        let path = root.join("rows.csv");
        fs::write(&path, "name,notes\nAda,\"csvneedle, quoted\"\n").unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap().unwrap();

        assert_eq!(doc.text, "name notes Ada csvneedle, quoted");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_binary_plist_values() {
        let root = unique_temp_dir("gfm-content-bplist");
        let path = root.join("settings.plist");
        let mut dictionary = plist::Dictionary::new();
        dictionary.insert("Owner".into(), plist::Value::String("plistneedle".into()));
        let mut bytes = Vec::new();
        plist::Value::Dictionary(dictionary)
            .to_writer_binary(&mut bytes)
            .unwrap();
        fs::write(&path, bytes).unwrap();

        let doc = Extractor::default().extract_path(&path).unwrap().unwrap();

        assert_eq!(doc.text, "Owner plistneedle");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn applies_office_entry_budget() {
        let root = unique_temp_dir("gfm-content-office-budget");
        let path = root.join("brief.docx");
        fs::write(
            &path,
            ooxml_package(&[("word/document.xml", "<w:t>large office text</w:t>")]),
        )
        .unwrap();
        let extractor = Extractor::new(ExtractionPolicy {
            max_office_entry_bytes: 4,
            ..ExtractionPolicy::default()
        });

        let doc = extractor.extract_path(&path).unwrap();

        assert!(doc.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cached_extractor_hits_for_unchanged_file_identity_and_signature() {
        let root = unique_temp_dir("gfm-content-cache-hit");
        let path = root.join("cache.md");
        fs::write(&path, "cached needle").unwrap();
        let record = record_for_path(&path);
        let mut cached = CachedExtractor::default();

        let first = cached.extract_record_report(&record).unwrap();
        let second = cached.extract_record_report(&record).unwrap();

        assert_eq!(first.status, ExtractionCacheStatus::Miss);
        assert_eq!(second.status, ExtractionCacheStatus::Hit);
        assert_eq!(first.key, second.key);
        assert_eq!(cached.cache_len(), 1);
        assert!(second.as_tsv().contains("status=hit"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cached_extractor_misses_after_content_signature_changes() {
        let root = unique_temp_dir("gfm-content-cache-content-change");
        let path = root.join("cache.md");
        fs::write(&path, "cached needle").unwrap();
        let mut record = record_for_path(&path);
        let mut cached = CachedExtractor::default();

        let first = cached.extract_record_report(&record).unwrap();
        fs::write(&path, "cached changed needle").unwrap();
        record = record_for_path(&path);
        let second = cached.extract_record_report(&record).unwrap();

        assert_eq!(first.status, ExtractionCacheStatus::Miss);
        assert_eq!(second.status, ExtractionCacheStatus::Miss);
        assert_ne!(first.key.content, second.key.content);
        assert_eq!(cached.cache_len(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cached_extractor_misses_after_metadata_epoch_changes() {
        let root = unique_temp_dir("gfm-content-cache-metadata-change");
        let path = root.join("cache.md");
        fs::write(&path, "cached needle").unwrap();
        let mut record = record_for_path(&path);
        let mut cached = CachedExtractor::default();

        let first = cached.extract_record_report(&record).unwrap();
        record.xattrs_digest = record.xattrs_digest.wrapping_add(1);
        let second = cached.extract_record_report(&record).unwrap();

        assert_eq!(first.status, ExtractionCacheStatus::Miss);
        assert_eq!(second.status, ExtractionCacheStatus::Miss);
        assert_ne!(first.key.metadata_epoch, second.key.metadata_epoch);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn quarantine_blocks_repeated_timeout_failures() {
        let root = unique_temp_dir("gfm-content-timeout-quarantine");
        let path = root.join("slow.pdf");
        fs::write(&path, minimal_pdf("slow")).unwrap();
        let fingerprint = ExtractionFingerprint::for_path(&path).unwrap();
        let mut quarantine = ExtractionQuarantine::new(2);

        assert_eq!(
            quarantine.record_failure(
                &path,
                &fingerprint,
                QuarantineFailureKind::Timeout,
                "worker-timeout"
            ),
            QuarantineDecision::Allow
        );
        let blocked = quarantine.record_failure(
            &path,
            &fingerprint,
            QuarantineFailureKind::Timeout,
            "worker-timeout",
        );

        assert!(matches!(blocked, QuarantineDecision::Quarantined(_)));
        assert!(blocked.as_tsv().contains("\treason=worker-timeout\t"));
        assert!(matches!(
            quarantine.before_extract(&path, &fingerprint),
            QuarantineDecision::Quarantined(_)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn quarantine_persists_crash_failures_across_restart() {
        let root = unique_temp_dir("gfm-content-crash-quarantine");
        let path = root.join("crash.docx");
        let store = root.join("quarantine.gfmquarantine");
        fs::write(
            &path,
            ooxml_package(&[("word/document.xml", "<w:t>crash</w:t>")]),
        )
        .unwrap();
        let fingerprint = ExtractionFingerprint::for_path(&path).unwrap();
        let mut quarantine = ExtractionQuarantine::new(1);
        let blocked = quarantine.record_failure(
            &path,
            &fingerprint,
            QuarantineFailureKind::Crash,
            "worker-crash",
        );

        assert!(matches!(blocked, QuarantineDecision::Quarantined(_)));
        quarantine.write(&store).unwrap();
        let reloaded = ExtractionQuarantine::read(&store).unwrap();

        assert!(matches!(
            reloaded.before_extract(&path, &fingerprint),
            QuarantineDecision::Quarantined(_)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "{}-{}",
            prefix,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn record_for_path(path: &Path) -> FileRecord {
        let metadata = fs::metadata(path).unwrap();
        FileRecord {
            id: FileId::new(VolumeId(1), 1),
            parent: None,
            path: path.to_path_buf(),
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            kind: FileKind::File,
            len: metadata.len(),
            mode: 0,
            owner: 0,
            group: 0,
            xattrs_digest: 0,
            created: metadata.created().ok(),
            modified: metadata.modified().ok(),
            changed: metadata.modified().ok(),
            hidden: false,
            tags: Vec::new(),
            finder_comment: None,
        }
    }

    fn minimal_pdf(text: &str) -> Vec<u8> {
        format!(
            "%PDF-1.4
1 0 obj
<< /Type /Page /Contents 2 0 R >>
endobj
2 0 obj
<< /Length {} >>
stream
BT /F1 12 Tf 72 720 Td ({}) Tj ET
endstream
endobj
%%EOF",
            text.len() + 31,
            text
        )
        .into_bytes()
    }

    fn multi_page_pdf(pages: usize) -> Vec<u8> {
        let mut pdf = b"%PDF-1.4\n".to_vec();
        for index in 0..pages {
            pdf.extend(format!("{index} 0 obj << /Type /Page >> endobj\n").as_bytes());
        }
        pdf.extend(b"%%EOF");
        pdf
    }

    fn ooxml_package(parts: &[(&str, &str)]) -> Vec<u8> {
        zip_package(parts)
    }

    fn zip_package(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, text) in parts {
            writer.start_file(*name, options).unwrap();
            writer.write_all(text.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }
}
