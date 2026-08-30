use crate::{IndexSnapshot, LiveIndex};
use gfm_content::{
    extractor_version_for_path, ExtractionFingerprint, ExtractionQuarantine, Extractor,
};
use gfm_jobs::Cancellation;
use gfm_store::{
    atomic_write_checked, compact_content_postings_with_segments_checked,
    compact_content_segments_checked, compact_content_segments_with_policy_checked,
    plan_content_segment_merge_checked, read_content_postings_checked,
    write_content_segment_checked, ContentArchiveCleanupAction, ContentArchiveCleanupPolicy,
    ContentArchiveManifest, ContentArchiveManifestEntry, ContentMergePolicy, ContentMergeTier,
};
use gfm_types::{
    ContentPosting, ContentSegment, FileId, FileKind, FileRecord, GfmError, Result, VolumeId,
};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentIndexOptions {
    pub batch_size: usize,
    pub segment_prefix: String,
}

impl Default for ContentIndexOptions {
    fn default() -> Self {
        Self {
            batch_size: 1024,
            segment_prefix: "content".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentIndexReport {
    pub indexed: usize,
    pub skipped: usize,
    pub quarantined: usize,
    pub unchanged: usize,
    pub tombstoned: usize,
    pub terms: usize,
    pub segments: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContentIndexBatchReport {
    pub indexed: usize,
    pub skipped: usize,
    pub quarantined: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct QuarantineContentIndexRequest<'a> {
    pub snapshot: &'a IndexSnapshot,
    pub previous_records: &'a [FileRecord],
    pub previous_content_path: Option<&'a Path>,
    pub segment_dir: &'a Path,
    pub content_path: &'a Path,
    pub cancellation: &'a Cancellation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentIndexDelta {
    pub records: Vec<FileRecord>,
    pub tombstones: Vec<FileId>,
    pub unchanged: usize,
}

impl ContentIndexDelta {
    pub fn from_records(current: &[FileRecord], previous: &[FileRecord]) -> Self {
        let previous_by_id = previous
            .iter()
            .map(|record| (record.id, record))
            .collect::<HashMap<_, _>>();
        let current_ids = current
            .iter()
            .map(|record| record.id)
            .collect::<HashSet<_>>();
        let mut records = Vec::new();
        let mut tombstones = Vec::new();
        let mut unchanged = 0;

        for record in current {
            match previous_by_id.get(&record.id) {
                Some(previous_record)
                    if content_record_signature(record)
                        == content_record_signature(previous_record) =>
                {
                    unchanged += 1;
                }
                Some(previous_record) => {
                    if previous_record.kind == FileKind::File {
                        tombstones.push(record.id);
                    }
                    records.push(record.clone());
                }
                None => records.push(record.clone()),
            }
        }

        for record in previous {
            if record.kind == FileKind::File && !current_ids.contains(&record.id) {
                tombstones.push(record.id);
            }
        }
        tombstones.sort();
        tombstones.dedup();

        Self {
            records,
            tombstones,
            unchanged,
        }
    }

    fn retry_quarantine_entries(
        &mut self,
        current: &[FileRecord],
        quarantine: &ExtractionQuarantine,
        cancellation: &Cancellation,
    ) -> Result<()> {
        let mut selected = self
            .records
            .iter()
            .map(|record| record.id)
            .collect::<HashSet<_>>();
        for record in current {
            cancellation.check()?;
            if record.kind != FileKind::File || selected.contains(&record.id) {
                continue;
            }
            let fingerprint =
                ExtractionFingerprint::for_path_checked(&record.path, || cancellation.check())?;
            if quarantine.has_entry(&record.path, &fingerprint) {
                self.records.push(record.clone());
                selected.insert(record.id);
                self.unchanged = self.unchanged.saturating_sub(1);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ContentRecordSignature {
    kind: FileKind,
    len: u64,
    modified_ns: Option<u128>,
    changed_ns: Option<u128>,
    extractor_version: u32,
}

fn content_record_signature(record: &FileRecord) -> ContentRecordSignature {
    ContentRecordSignature {
        kind: record.kind,
        len: record.len,
        modified_ns: system_time_ns(record.modified),
        changed_ns: system_time_ns(record.changed),
        extractor_version: extractor_version_for_path(&record.path),
    }
}

fn system_time_ns(value: Option<std::time::SystemTime>) -> Option<u128> {
    value
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContentMaintenanceOptions {
    pub merge_policy: ContentMergePolicy,
    pub cleanup_policy: ContentArchiveCleanupPolicy,
    pub cleanup_retired_archives: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentMaintenanceReport {
    pub scheduled: bool,
    pub terms: usize,
    pub merged_segments: Vec<PathBuf>,
    pub retained_segments: Vec<PathBuf>,
    pub published_archive: Option<PathBuf>,
    pub tier: ContentMergeTier,
    pub merge_bytes: u64,
    pub tombstone_segments: usize,
    pub manifest_archives: usize,
    pub removed_archives: Vec<PathBuf>,
    pub active_archives: Vec<PathBuf>,
    pub missing_archives: Vec<PathBuf>,
    pub cleanup_action: ContentArchiveCleanupAction,
    pub cleanup_bytes: u64,
    pub deferred_archives: Vec<PathBuf>,
    pub deferred_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentIndexJobSpec {
    pub root: PathBuf,
    pub segment_dir: PathBuf,
    pub records_path: PathBuf,
    pub content_path: PathBuf,
    pub volume: Option<VolumeId>,
    pub batch_size: usize,
}

impl ContentIndexJobSpec {
    pub fn new(
        root: impl Into<PathBuf>,
        segment_dir: impl Into<PathBuf>,
        records_path: impl Into<PathBuf>,
        content_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            root: root.into(),
            segment_dir: segment_dir.into(),
            records_path: records_path.into(),
            content_path: content_path.into(),
            volume: None,
            batch_size: ContentIndexOptions::default().batch_size,
        }
    }

    pub fn with_volume(mut self, volume: VolumeId) -> Self {
        self.volume = Some(volume);
        self
    }

    pub fn options(&self) -> ContentIndexOptions {
        ContentIndexOptions {
            batch_size: self.batch_size,
            segment_prefix: ContentIndexOptions::default().segment_prefix,
        }
    }

    pub fn write(&self, path: impl AsRef<Path>) -> Result<()> {
        self.write_checked(path, || Ok(()))
    }

    pub fn write_checked(
        &self,
        path: impl AsRef<Path>,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<()> {
        let path = path.as_ref();
        atomic_write_checked(path, &mut check_control, |writer, check_control| {
            let mut writer = BufWriter::new(writer);
            macro_rules! line {
                ($($arg:tt)*) => {
                    writeln!($($arg)*).map_err(|err| GfmError::io(path, err))?
                };
            }
            check_control()?;
            line!(writer, "gfm-content-job-v1");
            check_control()?;
            line!(writer, "root\t{}", escape_path(&self.root));
            line!(writer, "segment_dir\t{}", escape_path(&self.segment_dir));
            line!(writer, "records_path\t{}", escape_path(&self.records_path));
            line!(writer, "content_path\t{}", escape_path(&self.content_path));
            if let Some(volume) = self.volume {
                line!(writer, "volume_id\t{}", volume.0);
            }
            line!(writer, "batch_size\t{}", self.batch_size);
            check_control()?;
            writer.flush().map_err(|err| GfmError::io(path, err))
        })
        .map(|_| ())
    }

    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        Self::read_checked(path, || Ok(()))
    }

    pub fn read_checked(
        path: impl AsRef<Path>,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        let path = path.as_ref();
        check_control()?;
        let file = fs::File::open(path).map_err(|err| GfmError::io(path, err))?;
        check_control()?;
        let mut lines = BufReader::new(file).lines();
        match lines.next() {
            Some(Ok(header)) if header == "gfm-content-job-v1" => {}
            Some(Ok(header)) => {
                return Err(GfmError::Format(format!(
                    "unsupported content job header `{header}` in {}",
                    path.display()
                )))
            }
            Some(Err(err)) => return Err(GfmError::io(path, err)),
            None => {
                return Err(GfmError::Format(format!(
                    "empty content job {}",
                    path.display()
                )))
            }
        }
        check_control()?;

        let mut root = None;
        let mut segment_dir = None;
        let mut records_path = None;
        let mut content_path = None;
        let mut volume = None;
        let mut batch_size = None;
        for (line_index, line) in lines.enumerate() {
            check_control()?;
            let line = line.map_err(|err| GfmError::io(path, err))?;
            check_control()?;
            let (key, value) = line.split_once('\t').ok_or_else(|| {
                GfmError::Format(format!(
                    "{} line {}: expected key and value",
                    path.display(),
                    line_index + 2
                ))
            })?;
            match key {
                "root" => root = Some(PathBuf::from(unescape(value)?)),
                "segment_dir" => segment_dir = Some(PathBuf::from(unescape(value)?)),
                "records_path" => records_path = Some(PathBuf::from(unescape(value)?)),
                "content_path" => content_path = Some(PathBuf::from(unescape(value)?)),
                "volume_id" => {
                    volume = Some(VolumeId(value.parse().map_err(|err| {
                        GfmError::Format(format!("invalid content job volume id `{value}`: {err}"))
                    })?))
                }
                "batch_size" => {
                    batch_size = Some(value.parse().map_err(|err| {
                        GfmError::Format(format!("invalid content job batch size `{value}`: {err}"))
                    })?)
                }
                other => {
                    return Err(GfmError::Format(format!(
                        "{}: unknown content job field `{other}`",
                        path.display()
                    )))
                }
            }
        }
        check_control()?;

        Ok(Self {
            root: required_field(root, "root", path)?,
            segment_dir: required_field(segment_dir, "segment_dir", path)?,
            records_path: required_field(records_path, "records_path", path)?,
            content_path: required_field(content_path, "content_path", path)?,
            volume,
            batch_size: required_field(batch_size, "batch_size", path)?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct BackgroundContentIndexer {
    extractor: Extractor,
    options: ContentIndexOptions,
}

impl BackgroundContentIndexer {
    pub fn new(extractor: Extractor, options: ContentIndexOptions) -> Self {
        Self { extractor, options }
    }

    pub fn run_to_segments(
        &self,
        snapshot: &IndexSnapshot,
        output_dir: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<ContentIndexReport> {
        let output_dir = output_dir.as_ref();
        fs::create_dir_all(output_dir).map_err(|err| gfm_types::GfmError::io(output_dir, err))?;
        let batch_size = self.options.batch_size.max(1);
        let mut report = ContentIndexReport {
            indexed: 0,
            skipped: 0,
            quarantined: 0,
            unchanged: 0,
            tombstoned: 0,
            terms: 0,
            segments: Vec::new(),
        };

        for (batch_index, records) in snapshot.records.chunks(batch_size).enumerate() {
            cancellation.check()?;
            let segment_path = output_dir.join(format!(
                "{}-{:08}.gfmseg",
                self.options.segment_prefix, batch_index
            ));
            let mut live = LiveIndex::from_records(records.to_vec());
            let indexed = live.index_content_cancellable(&self.extractor, cancellation)?;
            report.indexed += indexed;
            report.skipped += records.len().saturating_sub(indexed);
            let postings = live.content_postings();
            report.terms += postings.len();
            write_content_segment_checked(
                &segment_path,
                &ContentSegment {
                    tombstones: Vec::new(),
                    postings,
                },
                || cancellation.check(),
            )?;
            report.segments.push(segment_path);
        }
        Ok(report)
    }

    pub fn run_incremental_to_segments(
        &self,
        snapshot: &IndexSnapshot,
        previous_records: &[FileRecord],
        output_dir: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<ContentIndexReport> {
        self.run_incremental_to_segments_with_quarantine(
            snapshot,
            previous_records,
            output_dir,
            cancellation,
            None,
        )
    }

    fn run_incremental_to_segments_with_quarantine(
        &self,
        snapshot: &IndexSnapshot,
        previous_records: &[FileRecord],
        output_dir: impl AsRef<Path>,
        cancellation: &Cancellation,
        mut quarantine: Option<&mut ExtractionQuarantine>,
    ) -> Result<ContentIndexReport> {
        let output_dir = output_dir.as_ref();
        fs::create_dir_all(output_dir).map_err(|err| gfm_types::GfmError::io(output_dir, err))?;
        let mut delta = ContentIndexDelta::from_records(&snapshot.records, previous_records);
        if let Some(quarantine) = quarantine.as_deref() {
            delta.retry_quarantine_entries(&snapshot.records, quarantine, cancellation)?;
        }
        let batch_size = self.options.batch_size.max(1);
        let mut report = ContentIndexReport {
            indexed: 0,
            skipped: 0,
            quarantined: 0,
            unchanged: delta.unchanged,
            tombstoned: delta.tombstones.len(),
            terms: 0,
            segments: Vec::new(),
        };

        if delta.records.is_empty() && !delta.tombstones.is_empty() {
            cancellation.check()?;
            let segment_path =
                output_dir.join(format!("{}-{:08}.gfmseg", self.options.segment_prefix, 0));
            write_content_segment_checked(
                &segment_path,
                &ContentSegment {
                    tombstones: delta.tombstones,
                    postings: Vec::new(),
                },
                || cancellation.check(),
            )?;
            report.segments.push(segment_path);
            return Ok(report);
        }

        for (batch_index, records) in delta.records.chunks(batch_size).enumerate() {
            cancellation.check()?;
            let segment_path = output_dir.join(format!(
                "{}-{:08}.gfmseg",
                self.options.segment_prefix, batch_index
            ));
            let mut live = LiveIndex::from_records(records.to_vec());
            let batch = match quarantine.as_deref_mut() {
                Some(quarantine) => live.index_content_with_quarantine_cancellable(
                    &self.extractor,
                    quarantine,
                    cancellation,
                )?,
                None => {
                    let indexed = live.index_content_cancellable(&self.extractor, cancellation)?;
                    ContentIndexBatchReport {
                        indexed,
                        skipped: records.len().saturating_sub(indexed),
                        quarantined: 0,
                    }
                }
            };
            report.indexed += batch.indexed;
            report.skipped += batch.skipped;
            report.quarantined += batch.quarantined;
            let postings = live.content_postings();
            report.terms += postings.len();
            write_content_segment_checked(
                &segment_path,
                &ContentSegment {
                    tombstones: if batch_index == 0 {
                        delta.tombstones.clone()
                    } else {
                        Vec::new()
                    },
                    postings,
                },
                || cancellation.check(),
            )?;
            report.segments.push(segment_path);
        }
        Ok(report)
    }

    pub fn run_and_compact(
        &self,
        snapshot: &IndexSnapshot,
        segment_dir: impl AsRef<Path>,
        content_path: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<ContentIndexReport> {
        let mut report = self.run_to_segments(snapshot, segment_dir, cancellation)?;
        cancellation.check()?;
        report.terms = compact_content_segments_checked(content_path, &report.segments, || {
            cancellation.check()
        })?
        .len();
        Ok(report)
    }

    pub fn run_incremental_and_compact(
        &self,
        snapshot: &IndexSnapshot,
        previous_records: &[FileRecord],
        previous_content_path: Option<&Path>,
        segment_dir: impl AsRef<Path>,
        content_path: impl AsRef<Path>,
        cancellation: &Cancellation,
    ) -> Result<ContentIndexReport> {
        let mut report = self.run_incremental_to_segments(
            snapshot,
            previous_records,
            segment_dir,
            cancellation,
        )?;
        cancellation.check()?;
        let base_postings =
            read_previous_content_postings_cancellable(previous_content_path, cancellation)?;
        report.terms = compact_content_postings_with_segments_checked(
            content_path,
            base_postings,
            &report.segments,
            || cancellation.check(),
        )?
        .len();
        Ok(report)
    }

    pub fn run_incremental_and_compact_with_quarantine(
        &self,
        request: QuarantineContentIndexRequest<'_>,
        quarantine: &mut ExtractionQuarantine,
    ) -> Result<ContentIndexReport> {
        let mut report = self.run_incremental_to_segments_with_quarantine(
            request.snapshot,
            request.previous_records,
            request.segment_dir,
            request.cancellation,
            Some(quarantine),
        )?;
        request.cancellation.check()?;
        let base_postings = read_previous_content_postings_cancellable(
            request.previous_content_path,
            request.cancellation,
        )?;
        report.terms = compact_content_postings_with_segments_checked(
            request.content_path,
            base_postings,
            &report.segments,
            || request.cancellation.check(),
        )?
        .len();
        Ok(report)
    }

    pub fn maintain_segments(
        &self,
        manifest_path: impl AsRef<Path>,
        output_archive: impl AsRef<Path>,
        segments: &[impl AsRef<Path>],
        options: &ContentMaintenanceOptions,
    ) -> Result<ContentMaintenanceReport> {
        self.maintain_segments_cancellable(
            manifest_path,
            output_archive,
            segments,
            options,
            &Cancellation::default(),
        )
    }

    pub fn maintain_segments_cancellable(
        &self,
        manifest_path: impl AsRef<Path>,
        output_archive: impl AsRef<Path>,
        segments: &[impl AsRef<Path>],
        options: &ContentMaintenanceOptions,
        cancellation: &Cancellation,
    ) -> Result<ContentMaintenanceReport> {
        let manifest_path = manifest_path.as_ref();
        let output_archive = output_archive.as_ref();
        let plan = plan_content_segment_merge_checked(segments, &options.merge_policy, || {
            cancellation.check()
        })?;
        if plan.merge_segments.is_empty() {
            return Ok(ContentMaintenanceReport {
                scheduled: false,
                terms: 0,
                merged_segments: Vec::new(),
                retained_segments: plan.retained_segments,
                published_archive: None,
                tier: plan.tier,
                merge_bytes: plan.merge_bytes,
                tombstone_segments: plan.tombstone_segments,
                manifest_archives: ContentArchiveManifest::read_checked(manifest_path, || {
                    cancellation.check()
                })?
                .archives
                .len(),
                removed_archives: Vec::new(),
                active_archives: Vec::new(),
                missing_archives: Vec::new(),
                cleanup_action: ContentArchiveCleanupAction::Skip,
                cleanup_bytes: 0,
                deferred_archives: Vec::new(),
                deferred_bytes: 0,
            });
        }

        let outcome = compact_content_segments_with_policy_checked(
            output_archive,
            &plan.merge_segments,
            &options.merge_policy,
            || cancellation.check(),
        )?;
        let manifest =
            ContentArchiveManifest::read_checked(manifest_path, || cancellation.check())?;
        let promotion = manifest.promote_archive(
            manifest_path,
            ContentArchiveManifestEntry {
                tier: outcome.tier,
                path: output_archive.to_path_buf(),
            },
            &[] as &[PathBuf],
        )?;
        promotion
            .manifest
            .write_checked(manifest_path, || cancellation.check())?;
        let cleanup_plan = promotion.manifest.plan_inactive_archive_cleanup(
            manifest_path,
            &promotion.retired_archives,
            &options.cleanup_policy,
        )?;
        let cleanup = if options.cleanup_retired_archives
            && cleanup_plan.action == ContentArchiveCleanupAction::Cleanup
        {
            promotion
                .manifest
                .cleanup_inactive_archives(manifest_path, &cleanup_plan.cleanup_archives)?
        } else {
            gfm_store::ContentArchiveCleanupReport {
                removed_archives: Vec::new(),
                active_archives: Vec::new(),
                missing_archives: Vec::new(),
            }
        };
        Ok(ContentMaintenanceReport {
            scheduled: true,
            terms: outcome.postings.len(),
            merged_segments: outcome.merged_segments,
            retained_segments: outcome.retained_segments,
            published_archive: Some(output_archive.to_path_buf()),
            tier: outcome.tier,
            merge_bytes: outcome.merge_bytes,
            tombstone_segments: outcome.tombstone_segments,
            manifest_archives: promotion.manifest.archives.len(),
            removed_archives: cleanup.removed_archives,
            active_archives: cleanup.active_archives,
            missing_archives: cleanup.missing_archives,
            cleanup_action: cleanup_plan.action,
            cleanup_bytes: cleanup_plan.cleanup_bytes,
            deferred_archives: cleanup_plan.deferred_archives,
            deferred_bytes: cleanup_plan.deferred_bytes,
        })
    }
}

impl Default for BackgroundContentIndexer {
    fn default() -> Self {
        Self::new(Extractor::default(), ContentIndexOptions::default())
    }
}

pub(crate) fn read_previous_content_postings_cancellable(
    path: Option<&Path>,
    cancellation: &Cancellation,
) -> Result<Vec<ContentPosting>> {
    cancellation.check()?;
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            cancellation.check()?;
            let postings = read_content_postings_checked(path, || cancellation.check())?;
            cancellation.check()?;
            Ok(postings)
        }
        Ok(_) => Ok(Vec::new()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(GfmError::io(
            path,
            format!("content postings metadata unavailable: {err}"),
        )),
    }
}

fn required_field<T>(value: Option<T>, field: &str, path: &Path) -> Result<T> {
    value.ok_or_else(|| {
        GfmError::Format(format!(
            "{}: missing content job field `{field}`",
            path.display()
        ))
    })
}

fn escape_path(path: &Path) -> String {
    escape(&path.to_string_lossy())
}

fn escape(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' => output.push_str("\\\\"),
            '\t' => output.push_str("\\t"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            other => output.push(other),
        }
    }
    output
}

fn unescape(input: &str) -> Result<String> {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        match chars.next() {
            Some('\\') => output.push('\\'),
            Some('t') => output.push('\t'),
            Some('n') => output.push('\n'),
            Some('r') => output.push('\r'),
            Some(other) => {
                return Err(GfmError::Format(format!(
                    "invalid content job escape `\\{other}`"
                )))
            }
            None => return Err(GfmError::Format("trailing content job escape".to_string())),
        }
    }
    Ok(output)
}
