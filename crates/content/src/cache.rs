use crate::{extractor_version_for_path, report::escape_report_field, ExtractionReport, Extractor};
use gfm_types::{FileId, FileRecord, GfmError, Result};
use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::UNIX_EPOCH;

const DEFAULT_MAX_ENTRIES: usize = 4096;
const DEFAULT_SAMPLE_BYTES: usize = 128 * 1024;
const SAMPLE_READ_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionCacheStatus {
    Hit,
    Miss,
}

impl ExtractionCacheStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hit => "hit",
            Self::Miss => "miss",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExtractionContentSignature {
    pub len: u64,
    pub modified_ns: Option<u128>,
    pub sample_hash: u64,
}

impl ExtractionContentSignature {
    pub fn for_path(path: &Path, len: u64, modified_ns: Option<u128>) -> Result<Self> {
        Self::for_path_checked(path, len, modified_ns, || Ok(()))
    }

    pub fn for_path_checked(
        path: &Path,
        len: u64,
        modified_ns: Option<u128>,
        check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        let sample_hash = sample_hash_checked(path, len, DEFAULT_SAMPLE_BYTES, check_control)?;
        Ok(Self {
            len,
            modified_ns,
            sample_hash,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExtractionCacheKey {
    pub file_id: FileId,
    pub extractor_version: u32,
    pub content: ExtractionContentSignature,
    pub metadata_epoch: u64,
}

impl ExtractionCacheKey {
    pub fn for_record(record: &FileRecord) -> Result<Self> {
        Self::for_record_checked(record, || Ok(()))
    }

    pub fn for_record_checked(
        record: &FileRecord,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        let modified_ns = system_time_ns(record.modified);
        check_control()?;
        Ok(Self {
            file_id: record.id,
            extractor_version: extractor_version_for_path(&record.path),
            content: ExtractionContentSignature::for_path_checked(
                &record.path,
                record.len,
                modified_ns,
                &mut check_control,
            )?,
            metadata_epoch: metadata_epoch(record),
        })
    }

    pub fn as_tsv_fields(&self) -> String {
        format!(
            "volume={}\tnode={}\tversion={}\tlen={}\tmodified-ns={}\tsample={:016x}\tmetadata-epoch={:016x}",
            self.file_id.volume.0,
            self.file_id.node,
            self.extractor_version,
            self.content.len,
            self.content
                .modified_ns
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.content.sample_hash,
            self.metadata_epoch
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedExtractionReport {
    pub status: ExtractionCacheStatus,
    pub key: ExtractionCacheKey,
    pub report: ExtractionReport,
}

impl CachedExtractionReport {
    pub fn as_tsv(&self) -> String {
        format!(
            "extract-cache\tstatus={}\t{}\tpath={}",
            self.status.as_str(),
            self.key.as_tsv_fields(),
            escape_report_field(&self.report.path.to_string_lossy())
        )
    }
}

#[derive(Debug, Clone)]
pub struct ExtractionCache {
    max_entries: usize,
    entries: BTreeMap<ExtractionCacheKey, ExtractionReport>,
    order: VecDeque<ExtractionCacheKey>,
}

impl ExtractionCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            max_entries: max_entries.max(1),
            entries: BTreeMap::new(),
            order: VecDeque::new(),
        }
    }

    pub fn get(&self, key: &ExtractionCacheKey) -> Option<ExtractionReport> {
        self.entries.get(key).cloned()
    }

    pub fn insert(&mut self, key: ExtractionCacheKey, report: ExtractionReport) {
        if !self.entries.contains_key(&key) {
            self.order.push_back(key.clone());
        }
        self.entries.insert(key, report);
        while self.entries.len() > self.max_entries {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for ExtractionCache {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_ENTRIES)
    }
}

#[derive(Debug, Clone)]
pub struct CachedExtractor {
    extractor: Extractor,
    cache: ExtractionCache,
}

impl CachedExtractor {
    pub fn new(extractor: Extractor, cache: ExtractionCache) -> Self {
        Self { extractor, cache }
    }

    pub fn extract_record_report(&mut self, record: &FileRecord) -> Result<CachedExtractionReport> {
        self.extract_record_report_checked(record, || Ok(()))
    }

    pub fn extract_record_report_checked(
        &mut self,
        record: &FileRecord,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<CachedExtractionReport> {
        let key = ExtractionCacheKey::for_record_checked(record, &mut check_control)?;
        check_control()?;
        if let Some(report) = self.cache.get(&key) {
            return Ok(CachedExtractionReport {
                status: ExtractionCacheStatus::Hit,
                key,
                report,
            });
        }
        check_control()?;
        let report = self
            .extractor
            .extract_path_report_checked(&record.path, &mut check_control)?;
        check_control()?;
        self.cache.insert(key.clone(), report.clone());
        Ok(CachedExtractionReport {
            status: ExtractionCacheStatus::Miss,
            key,
            report,
        })
    }

    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }
}

impl Default for CachedExtractor {
    fn default() -> Self {
        Self::new(Extractor::default(), ExtractionCache::default())
    }
}

fn sample_hash_checked(
    path: &Path,
    len: u64,
    max_sample_bytes: usize,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<u64> {
    check_control()?;
    let mut file = File::open(path).map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    let sample_budget = max_sample_bytes.max(2);
    if len <= sample_budget as u64 {
        let mut bytes = Vec::with_capacity(len as usize);
        read_to_end_checked(&mut file, path, &mut bytes, &mut check_control)?;
        return Ok(fnv1a64(&bytes));
    }

    let edge = sample_budget / 2;
    let mut bytes = Vec::with_capacity(sample_budget);
    let mut head = vec![0; edge];
    check_control()?;
    file.read_exact(&mut head)
        .map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    bytes.extend(head);
    file.seek(SeekFrom::End(-(edge as i64)))
        .map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    let mut tail = vec![0; edge];
    file.read_exact(&mut tail)
        .map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    bytes.extend(tail);
    Ok(fnv1a64(&bytes))
}

fn read_to_end_checked(
    file: &mut File,
    path: &Path,
    output: &mut Vec<u8>,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    let mut chunk = [0_u8; SAMPLE_READ_CHUNK_BYTES];
    loop {
        check_control()?;
        let read = file
            .read(&mut chunk)
            .map_err(|err| GfmError::io(path, err))?;
        check_control()?;
        if read == 0 {
            return Ok(());
        }
        output.extend_from_slice(&chunk[..read]);
    }
}

fn metadata_epoch(record: &FileRecord) -> u64 {
    let mut hash = FNV_OFFSET;
    hash = fnv1a64_u64(hash, u64::from(record.mode));
    hash = fnv1a64_u64(hash, u64::from(record.owner));
    hash = fnv1a64_u64(hash, u64::from(record.group));
    hash = fnv1a64_u64(hash, record.xattrs_digest);
    hash = fnv1a64_u64(hash, system_time_ns(record.changed).unwrap_or(0) as u64);
    hash
}

fn system_time_ns(value: Option<std::time::SystemTime>) -> Option<u128> {
    value
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
}

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn fnv1a64_u64(mut hash: u64, value: u64) -> u64 {
    for byte in value.to_le_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}
