use gfm_config::ConfigStore;
use gfm_content::Extractor;
use gfm_index::{Indexer, PersistentIndexPlan, PersistentIndexRecovery};
use gfm_jobs::Cancellation;
use gfm_store::{read_records_checked, ContentArchive};
use gfm_telemetry::{
    export_diagnostics_checked, DiagnosticExportError, DiagnosticPrivacy, IoSample, LatencyMetric,
    Telemetry,
};
use gfm_types::{FileKind, GfmError, Result};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebuildSpec {
    pub root: PathBuf,
    pub records_path: PathBuf,
    pub content_path: Option<PathBuf>,
}

impl RebuildSpec {
    pub fn records(root: impl Into<PathBuf>, records_path: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            records_path: records_path.into(),
            content_path: None,
        }
    }

    pub fn with_content(
        root: impl Into<PathBuf>,
        records_path: impl Into<PathBuf>,
        content_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            root: root.into(),
            records_path: records_path.into(),
            content_path: Some(content_path.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebuildReport {
    pub root: PathBuf,
    pub records_path: PathBuf,
    pub content_path: Option<PathBuf>,
    pub records: usize,
    pub inaccessible: usize,
    pub content_indexed: usize,
}

pub fn rebuild_index(spec: &RebuildSpec) -> Result<RebuildReport> {
    rebuild_index_cancellable(spec, &Cancellation::default())
}

pub fn rebuild_index_cancellable(
    spec: &RebuildSpec,
    cancellation: &Cancellation,
) -> Result<RebuildReport> {
    cancellation.check()?;
    let snapshot = Indexer::default().build_cancellable(&spec.root, cancellation)?;
    cancellation.check()?;
    let inaccessible = snapshot.inaccessible.len();
    let records = snapshot.records.len();
    let content_indexed = if let Some(content_path) = &spec.content_path {
        snapshot.save(&spec.records_path)?;
        cancellation.check()?;
        let mut live = snapshot.into_live();
        let indexed = live.index_content_cancellable(&Extractor::default(), cancellation)?;
        cancellation.check()?;
        live.save_content_postings(content_path)?;
        indexed
    } else {
        snapshot.save(&spec.records_path)?;
        0
    };
    cancellation.check()?;
    Ok(RebuildReport {
        root: spec.root.clone(),
        records_path: spec.records_path.clone(),
        content_path: spec.content_path.clone(),
        records,
        inaccessible,
        content_indexed,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistentIndexRecoverySpec {
    pub root: PathBuf,
    pub records_path: PathBuf,
    pub state_path: PathBuf,
    pub quarantine_dir: PathBuf,
}

impl PersistentIndexRecoverySpec {
    pub fn new(
        root: impl Into<PathBuf>,
        records_path: impl Into<PathBuf>,
        state_path: impl Into<PathBuf>,
        quarantine_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            root: root.into(),
            records_path: records_path.into(),
            state_path: state_path.into(),
            quarantine_dir: quarantine_dir.into(),
        }
    }
}

pub fn plan_index_recovery(spec: &PersistentIndexRecoverySpec) -> PersistentIndexPlan {
    Indexer::default().plan_persistent_recovery(&spec.root, &spec.records_path, &spec.state_path)
}

pub fn plan_index_recovery_cancellable(
    spec: &PersistentIndexRecoverySpec,
    cancellation: &Cancellation,
) -> Result<PersistentIndexPlan> {
    Indexer::default().plan_persistent_recovery_cancellable(
        &spec.root,
        &spec.records_path,
        &spec.state_path,
        cancellation,
    )
}

pub fn recover_index(spec: &PersistentIndexRecoverySpec) -> Result<PersistentIndexRecovery> {
    recover_index_cancellable(spec, &Cancellation::default())
}

pub fn recover_index_cancellable(
    spec: &PersistentIndexRecoverySpec,
    cancellation: &Cancellation,
) -> Result<PersistentIndexRecovery> {
    cancellation.check()?;
    Indexer::default().recover_persistent_cancellable(
        &spec.root,
        &spec.records_path,
        &spec.state_path,
        &spec.quarantine_dir,
        cancellation,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceExportReport {
    pub path: PathBuf,
    pub bytes_written: u64,
}

pub fn export_operator_trace(path: impl AsRef<Path>) -> Result<TraceExportReport> {
    export_operator_trace_checked(path, || Ok(()))
}

pub fn export_operator_trace_checked(
    path: impl AsRef<Path>,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<TraceExportReport> {
    let path = path.as_ref();
    check_control()?;
    let mut telemetry = Telemetry::default();
    telemetry.increment("operator_trace_export");
    telemetry.observe_latency(LatencyMetric::WindowRender, Duration::from_millis(1));
    telemetry.observe_io(IoSample {
        read_bytes: 0,
        written_bytes: 0,
        read_ops: 0,
        write_ops: 1,
    });
    check_control()?;
    let receipt =
        export_diagnostics_checked(path, &telemetry, DiagnosticPrivacy::default(), || {
            check_control().map_err(|err| match err {
                GfmError::Cancelled => DiagnosticExportError::Cancelled,
                other => DiagnosticExportError::Io {
                    path: path.to_path_buf(),
                    error: io::Error::other(other.to_string()),
                },
            })
        })
        .map_err(|err| match err {
            DiagnosticExportError::Cancelled => GfmError::Cancelled,
            other => GfmError::Format(other.to_string()),
        })?;
    Ok(TraceExportReport {
        path: receipt.path,
        bytes_written: receipt.bytes_written,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParityBaselineReport {
    pub config_path: PathBuf,
    pub baseline_root: PathBuf,
    pub macos_build: String,
    pub manifest_path: PathBuf,
}

pub fn select_parity_baseline(
    store: &ConfigStore,
    baseline_root: impl Into<PathBuf>,
    macos_build: impl Into<String>,
) -> Result<ParityBaselineReport> {
    select_parity_baseline_checked(store, baseline_root, macos_build, || Ok(()))
}

pub fn select_parity_baseline_checked(
    store: &ConfigStore,
    baseline_root: impl Into<PathBuf>,
    macos_build: impl Into<String>,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<ParityBaselineReport> {
    check_control()?;
    let baseline_root = baseline_root.into();
    let macos_build = macos_build.into();
    if baseline_root.as_os_str().is_empty() {
        return Err(GfmError::Format(
            "parity baseline root cannot be empty".to_string(),
        ));
    }
    if macos_build.trim().is_empty() {
        return Err(GfmError::Format(
            "parity macOS build cannot be empty".to_string(),
        ));
    }
    check_control()?;
    let manifest_path = validate_parity_baseline_manifest(&baseline_root, &macos_build)?;
    check_control()?;
    let mut config = store.load_or_create_default_checked(&mut check_control)?;
    check_control()?;
    config.parity.baseline_root = baseline_root.clone();
    config.parity.profile.macos_build = macos_build.clone();
    store.save_checked(&config, &mut check_control)?;
    check_control()?;
    Ok(ParityBaselineReport {
        config_path: store.path().to_path_buf(),
        baseline_root,
        macos_build,
        manifest_path,
    })
}

fn validate_parity_baseline_manifest(baseline_root: &Path, macos_build: &str) -> Result<PathBuf> {
    let manifest_path = baseline_root.join("manifest.tsv");
    let content = fs::read_to_string(&manifest_path).map_err(|err| {
        GfmError::io(
            &manifest_path,
            format!("parity baseline manifest unavailable: {err}"),
        )
    })?;
    let mut saw_matching_profile = false;
    for (line_index, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.first() != Some(&"profile") {
            continue;
        }
        let profile = BaselineManifestProfile::parse(line_index, &fields)?;
        if profile.macos_build == macos_build {
            saw_matching_profile = true;
            profile.validate(line_index)?;
            validate_baseline_fixture_manifest(baseline_root, &profile, line_index)?;
            break;
        }
    }
    if !saw_matching_profile {
        return Err(GfmError::Format(format!(
            "parity baseline manifest {} does not contain macOS build `{macos_build}`",
            manifest_path.display()
        )));
    }
    Ok(manifest_path)
}

fn validate_baseline_fixture_manifest(
    baseline_root: &Path,
    profile: &BaselineManifestProfile,
    line_index: usize,
) -> Result<()> {
    let fixture_manifest = resolve_baseline_manifest_path(baseline_root, &profile.fixture_manifest);
    let metadata = fs::metadata(&fixture_manifest).map_err(|err| {
        GfmError::io(
            &fixture_manifest,
            format!(
                "parity baseline manifest line {} fixture-manifest unavailable: {err}",
                line_index + 1
            ),
        )
    })?;
    if !metadata.is_file() {
        return Err(GfmError::Format(format!(
            "parity baseline manifest line {} fixture-manifest is not a file: {}",
            line_index + 1,
            fixture_manifest.display()
        )));
    }
    Ok(())
}

fn resolve_baseline_manifest_path(base: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BaselineManifestProfile {
    macos_build: String,
    fixture_manifest: String,
    captured_at: String,
    capture_command: String,
    reviewer: String,
    signer: String,
    approved_mask_set: String,
}

impl BaselineManifestProfile {
    fn parse(line_index: usize, fields: &[&str]) -> Result<Self> {
        let mut profile = Self {
            macos_build: String::new(),
            fixture_manifest: String::new(),
            captured_at: String::new(),
            capture_command: String::new(),
            reviewer: String::new(),
            signer: String::new(),
            approved_mask_set: String::new(),
        };
        for field in fields.iter().skip(1) {
            let Some((key, value)) = field.split_once('=') else {
                return Err(GfmError::Format(format!(
                    "parity baseline manifest line {} has invalid profile field `{field}`",
                    line_index + 1
                )));
            };
            match key {
                "macos-build" => profile.macos_build = value.to_string(),
                "fixture-manifest" => profile.fixture_manifest = value.to_string(),
                "captured-at" => profile.captured_at = value.to_string(),
                "capture-command" => profile.capture_command = value.to_string(),
                "reviewer" => profile.reviewer = value.to_string(),
                "signer" => profile.signer = value.to_string(),
                "approved-mask-set" => profile.approved_mask_set = value.to_string(),
                _ => {}
            }
        }
        Ok(profile)
    }

    fn validate(&self, line_index: usize) -> Result<()> {
        for (name, value) in [
            ("macos-build", self.macos_build.as_str()),
            ("fixture-manifest", self.fixture_manifest.as_str()),
            ("captured-at", self.captured_at.as_str()),
            ("capture-command", self.capture_command.as_str()),
            ("reviewer", self.reviewer.as_str()),
            ("signer", self.signer.as_str()),
            ("approved-mask-set", self.approved_mask_set.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(GfmError::Format(format!(
                    "parity baseline manifest line {} missing {name}",
                    line_index + 1
                )));
            }
        }
        if !is_valid_utc_capture_timestamp(&self.captured_at) {
            return Err(GfmError::Format(format!(
                "parity baseline manifest line {} has invalid captured-at `{}`",
                line_index + 1,
                self.captured_at
            )));
        }
        if !approved_mask_set_matches_macos_build(&self.approved_mask_set, &self.macos_build) {
            return Err(GfmError::Format(format!(
                "parity baseline manifest line {} has approved-mask-set `{}` that does not match macos-build `{}`",
                line_index + 1,
                self.approved_mask_set,
                self.macos_build
            )));
        }
        Ok(())
    }
}

fn approved_mask_set_matches_macos_build(mask_set: &str, macos_build: &str) -> bool {
    mask_set
        .split(|ch: char| !(ch.is_ascii_alphanumeric()))
        .any(|token| token == macos_build)
}

fn is_valid_utc_capture_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'Z'
    {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        if !matches!(index, 4 | 7 | 10 | 13 | 16 | 19) && !byte.is_ascii_digit() {
            return false;
        }
    }

    let year = parse_fixed_u32(bytes, 0, 4);
    let month = parse_fixed_u32(bytes, 5, 7);
    let day = parse_fixed_u32(bytes, 8, 10);
    let hour = parse_fixed_u32(bytes, 11, 13);
    let minute = parse_fixed_u32(bytes, 14, 16);
    let second = parse_fixed_u32(bytes, 17, 19);
    if year == 0 || !(1..=12).contains(&month) || hour > 23 || minute > 59 || second > 59 {
        return false;
    }

    let max_day = days_in_month(year, month);
    (1..=max_day).contains(&day)
}

fn parse_fixed_u32(bytes: &[u8], start: usize, end: usize) -> u32 {
    bytes[start..end]
        .iter()
        .fold(0, |value, byte| value * 10 + u32::from(byte - b'0'))
}

const fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

const fn is_leap_year(year: u32) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageInspection {
    Records(RecordInspection),
    Content(ContentInspection),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordInspection {
    pub path: PathBuf,
    pub bytes: u64,
    pub records: usize,
    pub files: usize,
    pub directories: usize,
    pub symlinks: usize,
    pub hidden: usize,
    pub tagged: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentInspection {
    pub path: PathBuf,
    pub bytes: u64,
    pub terms: usize,
}

pub fn inspect_storage(path: impl AsRef<Path>) -> Result<StorageInspection> {
    inspect_storage_checked(path, || Ok(()))
}

pub fn inspect_storage_checked(
    path: impl AsRef<Path>,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<StorageInspection> {
    let path = path.as_ref();
    check_control()?;
    let extension = path.extension().and_then(|extension| extension.to_str());
    match extension {
        Some("gfmidx") => {
            inspect_records_checked(path, &mut check_control).map(StorageInspection::Records)
        }
        Some("gfmcontent") => {
            inspect_content_checked(path, &mut check_control).map(StorageInspection::Content)
        }
        Some(other) => Err(GfmError::Format(format!(
            "{} has unsupported storage extension `{other}`",
            path.display()
        ))),
        None => Err(GfmError::Format(format!(
            "{} has no storage extension",
            path.display()
        ))),
    }
}

fn inspect_records_checked(
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<RecordInspection> {
    check_control()?;
    let records = read_records_checked(path, &mut check_control)?;
    check_control()?;
    let bytes = fs::metadata(path)
        .map_err(|err| GfmError::io(path, err))?
        .len();
    check_control()?;
    let mut files = 0;
    let mut directories = 0;
    let mut symlinks = 0;
    let mut hidden = 0;
    let mut tagged = 0;
    for record in &records {
        match record.kind {
            FileKind::File => files += 1,
            FileKind::Directory => directories += 1,
            FileKind::Symlink => symlinks += 1,
            FileKind::Other => {}
        }
        hidden += usize::from(record.hidden);
        tagged += usize::from(!record.tags.is_empty());
    }
    check_control()?;
    Ok(RecordInspection {
        path: path.to_path_buf(),
        bytes,
        records: records.len(),
        files,
        directories,
        symlinks,
        hidden,
        tagged,
    })
}

fn inspect_content_checked(
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<ContentInspection> {
    check_control()?;
    let archive = ContentArchive::open_checked(path, &mut check_control)?;
    check_control()?;
    let bytes = fs::metadata(path)
        .map_err(|err| GfmError::io(path, err))?
        .len();
    check_control()?;
    Ok(ContentInspection {
        path: path.to_path_buf(),
        bytes,
        terms: archive.indexed_terms(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn rebuilds_records_and_inspects_store() {
        let root = unique_temp_dir("gfm-diagnostics-rebuild");
        let records = root.join("records.gfmidx");
        fs::write(root.join("needle.md"), "needle").unwrap();

        let report = rebuild_index(&RebuildSpec::records(&root, &records)).unwrap();
        let inspection = inspect_storage(&records).unwrap();

        assert_eq!(report.records, 2);
        assert_eq!(report.content_indexed, 0);
        let StorageInspection::Records(inspection) = inspection else {
            panic!("expected record inspection");
        };
        assert_eq!(inspection.records, 2);
        assert_eq!(inspection.files, 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rebuilds_content_and_inspects_content_store() {
        let root = unique_temp_dir("gfm-diagnostics-content");
        let records = root.join("records.gfmidx");
        let content = root.join("content.gfmcontent");
        fs::write(root.join("needle.md"), "content needle").unwrap();

        let report = rebuild_index(&RebuildSpec::with_content(&root, &records, &content)).unwrap();
        let inspection = inspect_storage(&content).unwrap();

        assert_eq!(report.records, 2);
        assert_eq!(report.content_indexed, 1);
        let StorageInspection::Content(inspection) = inspection else {
            panic!("expected content inspection");
        };
        assert!(inspection.terms > 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancellable_rebuild_stops_before_publishing_records() {
        let root = unique_temp_dir("gfm-diagnostics-rebuild-cancel");
        let records = root.join("records.gfmidx");
        fs::write(root.join("needle.md"), "needle").unwrap();
        let cancellation = Cancellation::default();
        cancellation.cancel();

        let result =
            rebuild_index_cancellable(&RebuildSpec::records(&root, &records), &cancellation);

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(!records.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancellable_recovery_plan_stops_before_records_probe() {
        let root = unique_temp_dir("gfm-diagnostics-recovery-plan-cancel");
        let records = root.join("record-archive-unavailable".repeat(64));
        let state = root.join("state.gfmstate");
        let quarantine = root.join("quarantine");
        let cancellation = Cancellation::default();
        cancellation.cancel();

        let result = plan_index_recovery_cancellable(
            &PersistentIndexRecoverySpec::new(&root, &records, &state, &quarantine),
            &cancellation,
        );

        assert!(matches!(result, Err(GfmError::Cancelled)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancellable_recovery_stops_before_publishing_state() {
        let root = unique_temp_dir("gfm-diagnostics-recovery-cancel");
        let records = root.join("records.gfmidx");
        let state = root.join("state.gfmstate");
        let quarantine = root.join("quarantine");
        fs::write(root.join("needle.md"), "needle").unwrap();
        Indexer::default()
            .build_persistent(&root, &records, &state)
            .unwrap();
        fs::remove_file(&state).unwrap();
        let cancellation = Cancellation::default();
        cancellation.cancel();

        let result = recover_index_cancellable(
            &PersistentIndexRecoverySpec::new(&root, &records, &state, &quarantine),
            &cancellation,
        );

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(!state.exists());
        assert!(!quarantine.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exports_private_operator_trace() {
        let root = unique_temp_dir("gfm-diagnostics-trace");
        let trace = root.join("trace.json");

        let report = export_operator_trace(&trace).unwrap();

        assert_eq!(report.path, trace);
        assert!(report.bytes_written > 0);
        let encoded = fs::read_to_string(&report.path).unwrap();
        assert!(encoded.contains("\"schema_version\""));
        assert!(!encoded.contains("query_text"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancellable_trace_export_stops_before_writing() {
        let root = unique_temp_dir("gfm-diagnostics-trace-cancel");
        let trace = root.join("trace.json");

        let result = export_operator_trace_checked(&trace, || Err(GfmError::Cancelled));

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(!trace.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancellable_trace_export_preserves_existing_trace() {
        let root = unique_temp_dir("gfm-diagnostics-trace-preserve");
        let trace = root.join("trace.json");
        export_operator_trace(&trace).unwrap();
        let before = fs::read(&trace).unwrap();
        let mut checks = 0usize;

        let result = export_operator_trace_checked(&trace, || {
            checks += 1;
            if checks >= 4 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(checks >= 4);
        assert_eq!(fs::read(&trace).unwrap(), before);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selects_parity_baseline_in_config_store() {
        let root = unique_temp_dir("gfm-diagnostics-parity");
        let store = ConfigStore::new(root.join("config.toml"));
        write_parity_baseline_manifest(&root.join("baselines"), "25A354");

        let report = select_parity_baseline(&store, root.join("baselines"), "25A354").unwrap();
        let config = store.load().unwrap();

        assert_eq!(report.config_path, root.join("config.toml"));
        assert_eq!(config.parity.baseline_root, root.join("baselines"));
        assert_eq!(config.parity.profile.macos_build, "25A354");
        assert_eq!(report.manifest_path, root.join("baselines/manifest.tsv"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancellable_parity_baseline_stops_before_config_create() {
        let root = unique_temp_dir("gfm-diagnostics-parity-cancel-create");
        let store = ConfigStore::new(root.join("config.toml"));

        let result =
            select_parity_baseline_checked(&store, root.join("baselines"), "25A354", || {
                Err(GfmError::Cancelled)
            });

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(!store.path().exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancellable_parity_baseline_preserves_existing_config() {
        let root = unique_temp_dir("gfm-diagnostics-parity-cancel-preserve");
        let store = ConfigStore::new(root.join("config.toml"));
        write_parity_baseline_manifest(&root.join("baselines-a"), "25A354");
        write_parity_baseline_manifest(&root.join("baselines-b"), "25A355");
        select_parity_baseline(&store, root.join("baselines-a"), "25A354").unwrap();
        let before = fs::read(store.path()).unwrap();
        let mut checks = 0usize;

        let result =
            select_parity_baseline_checked(&store, root.join("baselines-b"), "25A355", || {
                checks += 1;
                if checks >= 8 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            });

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(checks >= 8);
        assert_eq!(fs::read(store.path()).unwrap(), before);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parity_baseline_selection_rejects_missing_manifest() {
        let root = unique_temp_dir("gfm-diagnostics-parity-missing-manifest");
        let store = ConfigStore::new(root.join("config.toml"));
        fs::create_dir_all(root.join("baselines")).unwrap();

        let err = select_parity_baseline(&store, root.join("baselines"), "25A354").unwrap_err();

        assert!(err.to_string().contains("parity baseline manifest"));
        assert!(!store.path().exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parity_baseline_selection_rejects_mismatched_build_manifest() {
        let root = unique_temp_dir("gfm-diagnostics-parity-mismatched-manifest");
        let store = ConfigStore::new(root.join("config.toml"));
        write_parity_baseline_manifest(&root.join("baselines"), "25B999");

        let err = select_parity_baseline(&store, root.join("baselines"), "25A354").unwrap_err();

        assert!(err
            .to_string()
            .contains("does not contain macOS build `25A354`"));
        assert!(!store.path().exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parity_baseline_selection_rejects_incomplete_manifest_profile() {
        let root = unique_temp_dir("gfm-diagnostics-parity-incomplete-manifest");
        let store = ConfigStore::new(root.join("config.toml"));
        fs::create_dir_all(root.join("baselines")).unwrap();
        fs::write(
            root.join("baselines/manifest.tsv"),
            "profile\tmacos-build=25A354\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\n",
        )
        .unwrap();

        let err = select_parity_baseline(&store, root.join("baselines"), "25A354").unwrap_err();

        assert!(err.to_string().contains("missing approved-mask-set"));
        assert!(!store.path().exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parity_baseline_selection_rejects_missing_fixture_manifest() {
        let root = unique_temp_dir("gfm-diagnostics-parity-missing-fixture-manifest");
        let store = ConfigStore::new(root.join("config.toml"));
        fs::create_dir_all(root.join("baselines")).unwrap();
        fs::write(
            root.join("baselines/manifest.tsv"),
            "profile\tmacos-build=25A354\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\n",
        )
        .unwrap();

        let err = select_parity_baseline(&store, root.join("baselines"), "25A354").unwrap_err();

        assert!(err.to_string().contains("fixture-manifest unavailable"));
        assert!(err.to_string().contains("fixtures/manifest.tsv"));
        assert!(!store.path().exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancellable_storage_inspection_honors_pre_cancelled_control() {
        let root = unique_temp_dir("gfm-diagnostics-storage-pre-cancel");
        let records = root.join("records.gfmidx");

        let result = inspect_storage_checked(&records, || Err(GfmError::Cancelled));

        assert!(matches!(result, Err(GfmError::Cancelled)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancellable_storage_inspection_can_cancel_during_records_read() {
        let root = unique_temp_dir("gfm-diagnostics-storage-record-cancel");
        let records = root.join("records.gfmidx");
        for index in 0..2_048 {
            fs::write(root.join(format!("needle-{index}.md")), "needle").unwrap();
        }
        Indexer::default()
            .build(&root)
            .unwrap()
            .save(&records)
            .unwrap();
        let mut checks = 0usize;

        let result = inspect_storage_checked(&records, || {
            checks += 1;
            if checks >= 8 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert!(checks >= 8);
        fs::remove_dir_all(root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "{}-{}-{}",
            prefix,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_parity_baseline_manifest(root: &Path, macos_build: &str) {
        fs::create_dir_all(root).unwrap();
        fs::create_dir_all(root.join("fixtures")).unwrap();
        fs::write(
            root.join("fixtures/manifest.tsv"),
            "scenario\troot\tfinder-view\tfiles\tdirectories\ntoolbar\tfixtures/toolbar\ticon\t1\t0\n",
        )
        .unwrap();
        fs::write(
            root.join("manifest.tsv"),
            format!(
                "profile\tmacos-build={macos_build}\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-{macos_build}-default\n"
            ),
        )
        .unwrap();
    }
}
