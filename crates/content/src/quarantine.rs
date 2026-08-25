use crate::{ExtractionFingerprint, ExtractionReport, ExtractionStatus};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

pub const EXTRACTION_QUARANTINE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuarantineFailureKind {
    Corrupt,
    Encrypted,
    Crash,
    Timeout,
}

impl QuarantineFailureKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Corrupt => "corrupt",
            Self::Encrypted => "encrypted",
            Self::Crash => "crash",
            Self::Timeout => "timeout",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "corrupt" => Some(Self::Corrupt),
            "encrypted" => Some(Self::Encrypted),
            "crash" => Some(Self::Crash),
            "timeout" => Some(Self::Timeout),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuarantineEntry {
    pub path: PathBuf,
    pub cache_key: String,
    pub kind: QuarantineFailureKind,
    pub reason: String,
    pub failures: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuarantineDecision {
    Allow,
    Quarantined(QuarantineEntry),
}

impl QuarantineDecision {
    pub fn as_tsv(&self) -> String {
        match self {
            Self::Allow => "quarantine\tallow".to_string(),
            Self::Quarantined(entry) => format!(
                "quarantine\tblocked\tpath={}\treason={}\tfailures={}\tcache-key={}",
                entry.path.display(),
                escape_field(&entry.reason),
                entry.failures,
                escape_field(&entry.cache_key)
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionQuarantine {
    failure_threshold: u32,
    entries: BTreeMap<String, QuarantineEntry>,
}

impl ExtractionQuarantine {
    pub fn new(failure_threshold: u32) -> Self {
        Self {
            failure_threshold: failure_threshold.max(1),
            entries: BTreeMap::new(),
        }
    }

    pub fn before_extract(
        &self,
        path: &Path,
        fingerprint: &ExtractionFingerprint,
    ) -> QuarantineDecision {
        let key = fingerprint.cache_key(path);
        match self.entries.get(&key) {
            Some(entry) if entry.failures >= self.failure_threshold => {
                QuarantineDecision::Quarantined(entry.clone())
            }
            _ => QuarantineDecision::Allow,
        }
    }

    pub fn has_entry(&self, path: &Path, fingerprint: &ExtractionFingerprint) -> bool {
        self.entries.contains_key(&fingerprint.cache_key(path))
    }

    pub fn record_report(&mut self, report: &ExtractionReport) -> QuarantineDecision {
        let key = report.fingerprint.cache_key(&report.path);
        match &report.status {
            ExtractionStatus::Quarantined(reason) => self.record_failure_key(
                report.path.clone(),
                key,
                report_failure_kind(reason),
                (*reason).to_string(),
            ),
            ExtractionStatus::Extracted | ExtractionStatus::Skipped(_) => {
                self.entries.remove(&key);
                QuarantineDecision::Allow
            }
        }
    }

    pub fn record_failure(
        &mut self,
        path: impl Into<PathBuf>,
        fingerprint: &ExtractionFingerprint,
        kind: QuarantineFailureKind,
        reason: impl Into<String>,
    ) -> QuarantineDecision {
        let path = path.into();
        let key = fingerprint.cache_key(&path);
        self.record_failure_key(path, key, kind, reason)
    }

    pub fn record_success(
        &mut self,
        path: &Path,
        fingerprint: &ExtractionFingerprint,
    ) -> QuarantineDecision {
        self.entries.remove(&fingerprint.cache_key(path));
        QuarantineDecision::Allow
    }

    pub fn write(&self, path: impl AsRef<Path>) -> crate::Result<()> {
        let path = path.as_ref();
        let temp = quarantine_temp_path(path);
        let file = File::create(&temp).map_err(|err| gfm_types::GfmError::io(&temp, err))?;
        let mut writer = BufWriter::new(file);
        writeln!(writer, "gfm-extraction-quarantine-v1")
            .map_err(|err| gfm_types::GfmError::io(&temp, err))?;
        writeln!(
            writer,
            "schema_version\t{EXTRACTION_QUARANTINE_SCHEMA_VERSION}"
        )
        .map_err(|err| gfm_types::GfmError::io(&temp, err))?;
        writeln!(writer, "failure_threshold\t{}", self.failure_threshold)
            .map_err(|err| gfm_types::GfmError::io(&temp, err))?;
        for entry in self.entries.values() {
            writeln!(
                writer,
                "entry\t{}\t{}\t{}\t{}\t{}",
                escape_field(&entry.cache_key),
                escape_field(&entry.path.to_string_lossy()),
                entry.kind.as_str(),
                entry.failures,
                escape_field(&entry.reason)
            )
            .map_err(|err| gfm_types::GfmError::io(&temp, err))?;
        }
        writer
            .flush()
            .map_err(|err| gfm_types::GfmError::io(&temp, err))?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|err| gfm_types::GfmError::io(&temp, err))?;
        fs::rename(&temp, path).map_err(|err| gfm_types::GfmError::io(path, err))
    }

    pub fn read(path: impl AsRef<Path>) -> crate::Result<Self> {
        let path = path.as_ref();
        let file = File::open(path).map_err(|err| gfm_types::GfmError::io(path, err))?;
        let mut lines = BufReader::new(file).lines();
        let header = lines
            .next()
            .transpose()
            .map_err(|err| gfm_types::GfmError::io(path, err))?
            .ok_or_else(|| quarantine_format_error(path, "missing header"))?;
        if header != "gfm-extraction-quarantine-v1" {
            return Err(quarantine_format_error(
                path,
                "unsupported quarantine header",
            ));
        }
        let mut schema_version = None;
        let mut failure_threshold = None;
        let mut entries = BTreeMap::new();
        for line in lines {
            let line = line.map_err(|err| gfm_types::GfmError::io(path, err))?;
            let mut parts = line.split('\t');
            match parts.next() {
                Some("schema_version") => {
                    schema_version = Some(parse_u32(parts.next(), path, "schema_version")?);
                }
                Some("failure_threshold") => {
                    failure_threshold = Some(parse_u32(parts.next(), path, "failure_threshold")?);
                }
                Some("entry") => {
                    let cache_key = unescape_field(required_part(parts.next(), path, "cache_key")?);
                    let entry_path = PathBuf::from(unescape_field(required_part(
                        parts.next(),
                        path,
                        "entry path",
                    )?));
                    let kind = QuarantineFailureKind::parse(required_part(
                        parts.next(),
                        path,
                        "failure kind",
                    )?)
                    .ok_or_else(|| quarantine_format_error(path, "unsupported failure kind"))?;
                    let failures = parse_u32(parts.next(), path, "failures")?;
                    let reason = unescape_field(required_part(parts.next(), path, "reason")?);
                    entries.insert(
                        cache_key.clone(),
                        QuarantineEntry {
                            path: entry_path,
                            cache_key,
                            kind,
                            reason,
                            failures,
                        },
                    );
                }
                Some("") | None => {}
                Some(_) => return Err(quarantine_format_error(path, "unknown quarantine row")),
            }
        }
        if schema_version != Some(EXTRACTION_QUARANTINE_SCHEMA_VERSION) {
            return Err(quarantine_format_error(
                path,
                "unsupported quarantine schema version",
            ));
        }
        Ok(Self {
            failure_threshold: failure_threshold.unwrap_or(2).max(1),
            entries,
        })
    }

    fn record_failure_key(
        &mut self,
        path: PathBuf,
        key: String,
        kind: QuarantineFailureKind,
        reason: impl Into<String>,
    ) -> QuarantineDecision {
        let reason = reason.into();
        let entry = self
            .entries
            .entry(key.clone())
            .or_insert_with(|| QuarantineEntry {
                path,
                cache_key: key,
                kind,
                reason: reason.clone(),
                failures: 0,
            });
        entry.kind = kind;
        entry.reason = reason;
        entry.failures = entry.failures.saturating_add(1);
        if entry.failures >= self.failure_threshold {
            QuarantineDecision::Quarantined(entry.clone())
        } else {
            QuarantineDecision::Allow
        }
    }
}

impl Default for ExtractionQuarantine {
    fn default() -> Self {
        Self::new(2)
    }
}

fn escape_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

fn unescape_field(value: &str) -> String {
    let mut output = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('t') => output.push('\t'),
                Some('n') => output.push('\n'),
                Some('\\') => output.push('\\'),
                Some(other) => {
                    output.push('\\');
                    output.push(other);
                }
                None => output.push('\\'),
            }
        } else {
            output.push(ch);
        }
    }
    output
}

fn report_failure_kind(reason: &str) -> QuarantineFailureKind {
    if reason.contains("encrypted") {
        QuarantineFailureKind::Encrypted
    } else {
        QuarantineFailureKind::Corrupt
    }
}

fn required_part<'a>(value: Option<&'a str>, path: &Path, name: &str) -> crate::Result<&'a str> {
    value.ok_or_else(|| quarantine_format_error(path, &format!("missing {name}")))
}

fn parse_u32(value: Option<&str>, path: &Path, name: &str) -> crate::Result<u32> {
    required_part(value, path, name)?
        .parse()
        .map_err(|_| quarantine_format_error(path, &format!("invalid {name}")))
}

fn quarantine_format_error(path: &Path, message: &str) -> gfm_types::GfmError {
    gfm_types::GfmError::Format(format!("{}: {message}", path.display()))
}

fn quarantine_temp_path(path: &Path) -> PathBuf {
    let mut temp_name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| "quarantine".into());
    let suffix = format!(
        ".tmp.{}.{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    );
    temp_name.push(suffix);
    path.with_file_name(temp_name)
}
