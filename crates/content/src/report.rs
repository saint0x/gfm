use crate::extractor_version_for_path;
use gfm_types::{GfmError, Result};
use std::path::{Path, PathBuf};

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
    pub(crate) fn from_metadata(metadata: &std::fs::Metadata, extractor_version: u32) -> Self {
        let modified_ns = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos());
        Self {
            extractor_version,
            len: metadata.len(),
            modified_ns,
        }
    }

    pub fn for_path(path: &Path) -> Result<Self> {
        let metadata = std::fs::metadata(path).map_err(|err| GfmError::io(path, err))?;
        Ok(Self::from_metadata(
            &metadata,
            extractor_version_for_path(path),
        ))
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
