use gfm_config::ConfigStore;
use gfm_content::Extractor;
use gfm_index::{Indexer, PersistentIndexPlan, PersistentIndexRecovery};
use gfm_jobs::Cancellation;
use gfm_store::{read_records, ContentArchive};
use gfm_telemetry::{export_diagnostics, DiagnosticPrivacy, IoSample, LatencyMetric, Telemetry};
use gfm_types::{FileKind, GfmError, Result};
use std::fs;
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
    let path = path.as_ref();
    let mut telemetry = Telemetry::default();
    telemetry.increment("operator_trace_export");
    telemetry.observe_latency(LatencyMetric::WindowRender, Duration::from_millis(1));
    telemetry.observe_io(IoSample {
        read_bytes: 0,
        written_bytes: 0,
        read_ops: 0,
        write_ops: 1,
    });
    let receipt = export_diagnostics(path, &telemetry, DiagnosticPrivacy::default())
        .map_err(|err| GfmError::Format(err.to_string()))?;
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
}

pub fn select_parity_baseline(
    store: &ConfigStore,
    baseline_root: impl Into<PathBuf>,
    macos_build: impl Into<String>,
) -> Result<ParityBaselineReport> {
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
    let mut config = store.load_or_create_default()?;
    config.parity.baseline_root = baseline_root.clone();
    config.parity.profile.macos_build = macos_build.clone();
    store.save(&config)?;
    Ok(ParityBaselineReport {
        config_path: store.path().to_path_buf(),
        baseline_root,
        macos_build,
    })
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
    let path = path.as_ref();
    let extension = path.extension().and_then(|extension| extension.to_str());
    match extension {
        Some("gfmidx") => inspect_records(path).map(StorageInspection::Records),
        Some("gfmcontent") => inspect_content(path).map(StorageInspection::Content),
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

fn inspect_records(path: &Path) -> Result<RecordInspection> {
    let records = read_records(path)?;
    let bytes = fs::metadata(path)
        .map_err(|err| GfmError::io(path, err))?
        .len();
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

fn inspect_content(path: &Path) -> Result<ContentInspection> {
    let archive = ContentArchive::open(path)?;
    let bytes = fs::metadata(path)
        .map_err(|err| GfmError::io(path, err))?
        .len();
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
    fn selects_parity_baseline_in_config_store() {
        let root = unique_temp_dir("gfm-diagnostics-parity");
        let store = ConfigStore::new(root.join("config.toml"));

        let report = select_parity_baseline(&store, root.join("baselines"), "25A354").unwrap();
        let config = store.load().unwrap();

        assert_eq!(report.config_path, root.join("config.toml"));
        assert_eq!(config.parity.baseline_root, root.join("baselines"));
        assert_eq!(config.parity.profile.macos_build, "25A354");
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
}
