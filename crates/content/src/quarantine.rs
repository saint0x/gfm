use crate::{ExtractionFingerprint, ExtractionReport, ExtractionStatus};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuarantineEntry {
    pub path: PathBuf,
    pub cache_key: String,
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

    pub fn record_report(&mut self, report: &ExtractionReport) -> QuarantineDecision {
        let key = report.fingerprint.cache_key(&report.path);
        match &report.status {
            ExtractionStatus::Quarantined(reason) => {
                let entry = self
                    .entries
                    .entry(key.clone())
                    .or_insert_with(|| QuarantineEntry {
                        path: report.path.clone(),
                        cache_key: key,
                        reason: (*reason).to_string(),
                        failures: 0,
                    });
                entry.reason = (*reason).to_string();
                entry.failures = entry.failures.saturating_add(1);
                if entry.failures >= self.failure_threshold {
                    QuarantineDecision::Quarantined(entry.clone())
                } else {
                    QuarantineDecision::Allow
                }
            }
            ExtractionStatus::Extracted | ExtractionStatus::Skipped(_) => {
                self.entries.remove(&key);
                QuarantineDecision::Allow
            }
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
