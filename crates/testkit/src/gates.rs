use crate::{
    run_macrobench, MacrobenchOptions, MacrobenchReport, MacrobenchScenario, MacrobenchStage,
};
use gfm_index::{
    Indexer, LiveIndex, SearchArchiveLookup, SearchLookupBudget, SearchLookupTelemetry,
};
use gfm_store::{
    fuzzy_postings_from_records, prefix_postings_from_records, substring_postings_from_records,
    write_fuzzy_postings, write_prefix_postings, write_substring_postings,
};
use gfm_telemetry::{BudgetViolation, FrameTimingSummary, ResourceSummary};
use gfm_types::{FileId, FileKind, FileRecord, GfmError, Result, VolumeId};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, PartialEq)]
pub struct RegressionInputs<'a> {
    pub macrobench: &'a MacrobenchReport,
    pub resources: Option<ResourceSummary>,
    pub frame_timing: Option<FrameTimingSummary>,
    pub index_size_bytes: Option<u64>,
    pub sidecar_lookup: Option<SearchLookupTelemetry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegressionGateOptions {
    pub max_peak_memory_bytes: u64,
    pub max_index_bytes_per_record: u64,
    pub max_sidecar_prefix_candidate_ids: usize,
    pub max_sidecar_fuzzy_verified_candidates: usize,
    pub fail_on_sidecar_truncation: bool,
    pub fail_on_frame_stalls: bool,
}

impl Default for RegressionGateOptions {
    fn default() -> Self {
        Self {
            max_peak_memory_bytes: 512 * 1024 * 1024,
            max_index_bytes_per_record: 512,
            max_sidecar_prefix_candidate_ids: 8_192,
            max_sidecar_fuzzy_verified_candidates: 8_192,
            fail_on_sidecar_truncation: true,
            fail_on_frame_stalls: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegressionGateReport {
    pub violations: Vec<RegressionGateViolation>,
}

impl RegressionGateReport {
    pub fn passed(&self) -> bool {
        self.violations.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RegressionGateViolation {
    Latency(BudgetViolation),
    MemoryPeakExceeded {
        observed_bytes: u64,
        budget_bytes: u64,
    },
    IndexSizeExceeded {
        observed_bytes_per_record: u64,
        budget_bytes_per_record: u64,
        records: u64,
        index_size_bytes: u64,
    },
    FrameTimeExceeded {
        observed_ns: u64,
        budget_ns: u64,
    },
    FrameStallDetected {
        stalls: u64,
    },
    SidecarPrefixCandidatesExceeded {
        observed: usize,
        budget: usize,
    },
    SidecarFuzzyCandidatesExceeded {
        observed: usize,
        budget: usize,
    },
    SidecarLookupTruncated {
        prefix_terms: usize,
        substring_terms: usize,
        fuzzy_terms_with_truncated_keys: usize,
        fuzzy_keys_with_truncated_terms: usize,
        fuzzy_candidate_terms: usize,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegressionGateRun {
    pub macrobench: MacrobenchReport,
    pub gate: RegressionGateReport,
    pub index_size_bytes: u64,
    pub sidecar_lookup: SearchLookupTelemetry,
}

impl RegressionGateRun {
    pub fn passed(&self) -> bool {
        self.gate.passed()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LargeSidecarGateOptions {
    pub workspace: PathBuf,
    pub records: usize,
    pub query: String,
    pub limit: usize,
    pub budget: SearchLookupBudget,
    pub thresholds: LargeSidecarThresholds,
}

impl LargeSidecarGateOptions {
    pub fn new(workspace: impl Into<PathBuf>, records: usize) -> Self {
        Self {
            workspace: workspace.into(),
            records,
            query: "packageproject00000006".to_string(),
            limit: 50,
            budget: SearchLookupBudget::default(),
            thresholds: LargeSidecarThresholds::production_macos_million(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LargeSidecarThresholds {
    pub profile: &'static str,
    pub min_required_ci_records: usize,
    pub max_prefix_bytes_per_record: u64,
    pub max_substring_bytes_per_record: u64,
    pub max_fuzzy_bytes_per_record: u64,
    pub max_prefix_candidate_ids_per_run: usize,
    pub max_substring_candidate_ids_per_run: usize,
    pub max_fuzzy_verified_candidates_per_run: usize,
    pub require_zero_truncation: bool,
}

impl LargeSidecarThresholds {
    pub const fn production_macos_million() -> Self {
        Self {
            profile: "production-macos-million-v1",
            min_required_ci_records: 1_000_000,
            max_prefix_bytes_per_record: 256,
            max_substring_bytes_per_record: 512,
            max_fuzzy_bytes_per_record: 4096,
            max_prefix_candidate_ids_per_run: 4096,
            max_substring_candidate_ids_per_run: 4096,
            max_fuzzy_verified_candidates_per_run: 4096,
            require_zero_truncation: true,
        }
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "large-sidecar-thresholds\tprofile={}\tmin-ci-records={}\tmax-prefix-bytes-per-record={}\tmax-substring-bytes-per-record={}\tmax-fuzzy-bytes-per-record={}\tmax-prefix-candidates-per-run={}\tmax-substring-candidates-per-run={}\tmax-fuzzy-verified-per-run={}\trequire-zero-truncation={}",
            self.profile,
            self.min_required_ci_records,
            self.max_prefix_bytes_per_record,
            self.max_substring_bytes_per_record,
            self.max_fuzzy_bytes_per_record,
            self.max_prefix_candidate_ids_per_run,
            self.max_substring_candidate_ids_per_run,
            self.max_fuzzy_verified_candidates_per_run,
            self.require_zero_truncation
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LargeSidecarGateReport {
    pub fixture_root: PathBuf,
    pub thresholds_path: PathBuf,
    pub history_path: PathBuf,
    pub thresholds: LargeSidecarThresholds,
    pub records: usize,
    pub probe_records: usize,
    pub prefix_keys: usize,
    pub substring_keys: usize,
    pub fuzzy_keys: usize,
    pub prefix_bytes: u64,
    pub substring_bytes: u64,
    pub fuzzy_bytes: u64,
    pub lookup: SearchLookupTelemetry,
    pub violations: Vec<LargeSidecarGateViolation>,
    pub passed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LargeSidecarGateViolation {
    PrefixBytesPerRecordExceeded {
        observed: u64,
        budget: u64,
    },
    SubstringBytesPerRecordExceeded {
        observed: u64,
        budget: u64,
    },
    FuzzyBytesPerRecordExceeded {
        observed: u64,
        budget: u64,
    },
    PrefixCandidatesExceeded {
        observed: usize,
        budget: usize,
    },
    SubstringCandidatesExceeded {
        observed: usize,
        budget: usize,
    },
    FuzzyVerifiedExceeded {
        observed: usize,
        budget: usize,
    },
    LookupTruncated {
        prefix_terms: usize,
        substring_terms: usize,
        fuzzy_terms_with_truncated_keys: usize,
        fuzzy_keys_with_truncated_terms: usize,
        fuzzy_candidate_terms: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LargeSidecarRunMetrics {
    records: usize,
    probe_records: usize,
    prefix_keys: usize,
    substring_keys: usize,
    fuzzy_keys: usize,
    prefix_bytes: u64,
    substring_bytes: u64,
    fuzzy_bytes: u64,
    violations: usize,
    passed: bool,
}

pub fn run_large_sidecar_gate(options: &LargeSidecarGateOptions) -> Result<LargeSidecarGateReport> {
    fs::create_dir_all(&options.workspace).map_err(|err| GfmError::io(&options.workspace, err))?;
    let fixture_root = options.workspace.join("gfm-large-sidecar-gate");
    if fixture_root.exists() {
        fs::remove_dir_all(&fixture_root).map_err(|err| GfmError::io(&fixture_root, err))?;
    }
    fs::create_dir_all(&fixture_root).map_err(|err| GfmError::io(&fixture_root, err))?;

    let records = realistic_large_records(options.records);
    let thresholds_path = fixture_root.join("thresholds.tsv");
    let history_path = options.workspace.join("gfm-large-sidecar-history.tsv");
    let prefix_path = fixture_root.join("records.gfmprefix");
    let substring_path = fixture_root.join("records.gfmsubstr");
    let fuzzy_path = fixture_root.join("records.gfmfuzzy");
    write_large_sidecar_thresholds(&thresholds_path, &options.thresholds)?;
    write_prefix_postings(&prefix_path, &prefix_postings_from_records(&records))?;
    write_substring_postings(&substring_path, &substring_postings_from_records(&records))?;
    write_fuzzy_postings(&fuzzy_path, &fuzzy_postings_from_records(&records))?;
    let probe_records = records
        .iter()
        .take((options.budget.max_prefix_ids_per_term * 2).max(options.limit))
        .cloned()
        .collect::<Vec<_>>();
    drop(records);
    let lookup = SearchArchiveLookup::open(&prefix_path, &substring_path, &fuzzy_path)?;
    let live = LiveIndex::from_records_deferred_sidecars(probe_records);
    let mut telemetry = SearchLookupTelemetry::default();
    for _ in 0..2 {
        let report =
            live.search_with_lookup_budget(&options.query, options.limit, &lookup, options.budget)?;
        telemetry.merge(&report.lookup);
    }

    let prefix_bytes = fs::metadata(&prefix_path)
        .map_err(|err| GfmError::io(&prefix_path, err))?
        .len();
    let substring_bytes = fs::metadata(&substring_path)
        .map_err(|err| GfmError::io(&substring_path, err))?
        .len();
    let fuzzy_bytes = fs::metadata(&fuzzy_path)
        .map_err(|err| GfmError::io(&fuzzy_path, err))?
        .len();
    let violations = evaluate_large_sidecar_gate(
        options.records,
        prefix_bytes,
        substring_bytes,
        fuzzy_bytes,
        &telemetry,
        &options.thresholds,
    );
    let passed = violations.is_empty();
    let metrics = LargeSidecarRunMetrics {
        records: options.records,
        probe_records: live.indexed_records(),
        prefix_keys: lookup.indexed_prefixes(),
        substring_keys: lookup.indexed_substring_grams(),
        fuzzy_keys: lookup.indexed_fuzzy_keys(),
        prefix_bytes,
        substring_bytes,
        fuzzy_bytes,
        violations: violations.len(),
        passed,
    };
    append_large_sidecar_history(&history_path, &options.thresholds, &metrics, &telemetry)?;

    Ok(LargeSidecarGateReport {
        fixture_root,
        thresholds_path,
        history_path,
        thresholds: options.thresholds.clone(),
        records: options.records,
        probe_records: live.indexed_records(),
        prefix_keys: lookup.indexed_prefixes(),
        substring_keys: lookup.indexed_substring_grams(),
        fuzzy_keys: lookup.indexed_fuzzy_keys(),
        prefix_bytes,
        substring_bytes,
        fuzzy_bytes,
        lookup: telemetry,
        violations,
        passed,
    })
}

pub fn run_regression_gate(
    macrobench_options: &MacrobenchOptions,
    gate_options: RegressionGateOptions,
) -> Result<RegressionGateRun> {
    let macrobench = run_macrobench(macrobench_options)?;
    let index_size_bytes = materialize_record_indexes(&macrobench)?;
    let sidecar_lookup = measure_sidecar_lookup(&macrobench)?;
    let gate = evaluate_regression_gate(
        &RegressionInputs {
            macrobench: &macrobench,
            resources: None,
            frame_timing: None,
            index_size_bytes: Some(index_size_bytes),
            sidecar_lookup: Some(sidecar_lookup.clone()),
        },
        gate_options,
    );
    Ok(RegressionGateRun {
        macrobench,
        gate,
        index_size_bytes,
        sidecar_lookup,
    })
}

pub fn evaluate_regression_gate(
    inputs: &RegressionInputs<'_>,
    options: RegressionGateOptions,
) -> RegressionGateReport {
    let mut violations = inputs
        .macrobench
        .budget_violations
        .iter()
        .copied()
        .map(RegressionGateViolation::Latency)
        .collect::<Vec<_>>();

    if let Some(resources) = &inputs.resources {
        let observed = resources.memory.peak_resident_bytes;
        if observed > options.max_peak_memory_bytes {
            violations.push(RegressionGateViolation::MemoryPeakExceeded {
                observed_bytes: observed,
                budget_bytes: options.max_peak_memory_bytes,
            });
        }
    }

    if let Some(index_size_bytes) = inputs.index_size_bytes {
        let records = inputs
            .macrobench
            .measurements
            .iter()
            .filter(|measurement| measurement.stage == MacrobenchStage::IndexBuild)
            .map(|measurement| measurement.records as u64)
            .sum::<u64>();
        if records > 0 {
            let bytes_per_record = index_size_bytes.div_ceil(records);
            if bytes_per_record > options.max_index_bytes_per_record {
                violations.push(RegressionGateViolation::IndexSizeExceeded {
                    observed_bytes_per_record: bytes_per_record,
                    budget_bytes_per_record: options.max_index_bytes_per_record,
                    records,
                    index_size_bytes,
                });
            }
        }
    }

    if let Some(frame_timing) = inputs.frame_timing {
        if let Some(p99) = frame_timing.histogram.p99 {
            if p99 > frame_timing.stall_threshold {
                violations.push(RegressionGateViolation::FrameTimeExceeded {
                    observed_ns: duration_ns(p99),
                    budget_ns: duration_ns(frame_timing.stall_threshold),
                });
            }
        }
        if options.fail_on_frame_stalls && frame_timing.stall_count > 0 {
            violations.push(RegressionGateViolation::FrameStallDetected {
                stalls: frame_timing.stall_count,
            });
        }
    }

    if let Some(sidecar) = &inputs.sidecar_lookup {
        if sidecar.prefix_candidate_ids > options.max_sidecar_prefix_candidate_ids {
            violations.push(RegressionGateViolation::SidecarPrefixCandidatesExceeded {
                observed: sidecar.prefix_candidate_ids,
                budget: options.max_sidecar_prefix_candidate_ids,
            });
        }
        if sidecar.fuzzy_verified_candidates > options.max_sidecar_fuzzy_verified_candidates {
            violations.push(RegressionGateViolation::SidecarFuzzyCandidatesExceeded {
                observed: sidecar.fuzzy_verified_candidates,
                budget: options.max_sidecar_fuzzy_verified_candidates,
            });
        }
        if sidecar.substring_candidate_ids > options.max_sidecar_prefix_candidate_ids {
            violations.push(RegressionGateViolation::SidecarPrefixCandidatesExceeded {
                observed: sidecar.substring_candidate_ids,
                budget: options.max_sidecar_prefix_candidate_ids,
            });
        }
        if options.fail_on_sidecar_truncation
            && (sidecar.prefix_truncated_terms > 0
                || sidecar.substring_term_truncated_grams > 0
                || sidecar.substring_truncated_grams > 0
                || sidecar.fuzzy_term_truncated_keys > 0
                || sidecar.fuzzy_key_truncated_terms > 0
                || sidecar.fuzzy_candidate_truncated_terms > 0)
        {
            violations.push(RegressionGateViolation::SidecarLookupTruncated {
                prefix_terms: sidecar.prefix_truncated_terms,
                substring_terms: sidecar.substring_term_truncated_grams
                    + sidecar.substring_truncated_grams,
                fuzzy_terms_with_truncated_keys: sidecar.fuzzy_term_truncated_keys,
                fuzzy_keys_with_truncated_terms: sidecar.fuzzy_key_truncated_terms,
                fuzzy_candidate_terms: sidecar.fuzzy_candidate_truncated_terms,
            });
        }
    }

    RegressionGateReport { violations }
}

fn duration_ns(value: std::time::Duration) -> u64 {
    value.as_nanos().min(u128::from(u64::MAX)) as u64
}

fn evaluate_large_sidecar_gate(
    records: usize,
    prefix_bytes: u64,
    substring_bytes: u64,
    fuzzy_bytes: u64,
    telemetry: &SearchLookupTelemetry,
    thresholds: &LargeSidecarThresholds,
) -> Vec<LargeSidecarGateViolation> {
    let mut violations = Vec::new();
    let records = records.max(1) as u64;
    let prefix_bytes_per_record = prefix_bytes.div_ceil(records);
    let substring_bytes_per_record = substring_bytes.div_ceil(records);
    let fuzzy_bytes_per_record = fuzzy_bytes.div_ceil(records);
    if prefix_bytes_per_record > thresholds.max_prefix_bytes_per_record {
        violations.push(LargeSidecarGateViolation::PrefixBytesPerRecordExceeded {
            observed: prefix_bytes_per_record,
            budget: thresholds.max_prefix_bytes_per_record,
        });
    }
    if substring_bytes_per_record > thresholds.max_substring_bytes_per_record {
        violations.push(LargeSidecarGateViolation::SubstringBytesPerRecordExceeded {
            observed: substring_bytes_per_record,
            budget: thresholds.max_substring_bytes_per_record,
        });
    }
    if fuzzy_bytes_per_record > thresholds.max_fuzzy_bytes_per_record {
        violations.push(LargeSidecarGateViolation::FuzzyBytesPerRecordExceeded {
            observed: fuzzy_bytes_per_record,
            budget: thresholds.max_fuzzy_bytes_per_record,
        });
    }
    if telemetry.prefix_candidate_ids > thresholds.max_prefix_candidate_ids_per_run * 2 {
        violations.push(LargeSidecarGateViolation::PrefixCandidatesExceeded {
            observed: telemetry.prefix_candidate_ids,
            budget: thresholds.max_prefix_candidate_ids_per_run * 2,
        });
    }
    if telemetry.substring_candidate_ids > thresholds.max_substring_candidate_ids_per_run * 2 {
        violations.push(LargeSidecarGateViolation::SubstringCandidatesExceeded {
            observed: telemetry.substring_candidate_ids,
            budget: thresholds.max_substring_candidate_ids_per_run * 2,
        });
    }
    if telemetry.fuzzy_verified_candidates > thresholds.max_fuzzy_verified_candidates_per_run * 2 {
        violations.push(LargeSidecarGateViolation::FuzzyVerifiedExceeded {
            observed: telemetry.fuzzy_verified_candidates,
            budget: thresholds.max_fuzzy_verified_candidates_per_run * 2,
        });
    }
    if thresholds.require_zero_truncation
        && (telemetry.prefix_truncated_terms > 0
            || telemetry.substring_term_truncated_grams > 0
            || telemetry.substring_truncated_grams > 0
            || telemetry.fuzzy_term_truncated_keys > 0
            || telemetry.fuzzy_key_truncated_terms > 0
            || telemetry.fuzzy_candidate_truncated_terms > 0)
    {
        violations.push(LargeSidecarGateViolation::LookupTruncated {
            prefix_terms: telemetry.prefix_truncated_terms,
            substring_terms: telemetry.substring_term_truncated_grams
                + telemetry.substring_truncated_grams,
            fuzzy_terms_with_truncated_keys: telemetry.fuzzy_term_truncated_keys,
            fuzzy_keys_with_truncated_terms: telemetry.fuzzy_key_truncated_terms,
            fuzzy_candidate_terms: telemetry.fuzzy_candidate_truncated_terms,
        });
    }
    violations
}

fn write_large_sidecar_thresholds(
    path: &PathBuf,
    thresholds: &LargeSidecarThresholds,
) -> Result<()> {
    fs::write(path, format!("{}\n", thresholds.as_tsv())).map_err(|err| GfmError::io(path, err))
}

fn append_large_sidecar_history(
    path: &PathBuf,
    thresholds: &LargeSidecarThresholds,
    metrics: &LargeSidecarRunMetrics,
    telemetry: &SearchLookupTelemetry,
) -> Result<()> {
    let existed = path.exists();
    let run = if existed {
        fs::read_to_string(path)
            .map_err(|err| GfmError::io(path, err))?
            .lines()
            .filter(|line| line.starts_with("large-sidecar-history\t"))
            .count()
            + 1
    } else {
        1
    };
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|err| GfmError::io(path, err))?;
    if !existed {
        writeln!(file, "{}", thresholds.as_tsv()).map_err(|err| GfmError::io(path, err))?;
    }
    let safe_records = metrics.records.max(1) as u64;
    writeln!(
        file,
        "large-sidecar-history\trun={run}\tprofile={}\trecords={}\tprobe-records={}\tprefix-keys={}\tsubstring-keys={}\tfuzzy-keys={}\tprefix-bytes={}\tsubstring-bytes={}\tfuzzy-bytes={}\tprefix-bytes-per-record={}\tsubstring-bytes-per-record={}\tfuzzy-bytes-per-record={}\tprefix-candidates={}\tsubstring-candidates={}\tfuzzy-verified={}\tprefix-cache-hits={}\tsubstring-cache-hits={}\tfuzzy-cache-hits={}\tprefix-cutoffs={}\tprefix-truncated={}\tsubstring-truncated={}\tfuzzy-truncated={}\tviolations={}\tpassed={}",
        thresholds.profile,
        metrics.records,
        metrics.probe_records,
        metrics.prefix_keys,
        metrics.substring_keys,
        metrics.fuzzy_keys,
        metrics.prefix_bytes,
        metrics.substring_bytes,
        metrics.fuzzy_bytes,
        metrics.prefix_bytes.div_ceil(safe_records),
        metrics.substring_bytes.div_ceil(safe_records),
        metrics.fuzzy_bytes.div_ceil(safe_records),
        telemetry.prefix_candidate_ids,
        telemetry.substring_candidate_ids,
        telemetry.fuzzy_verified_candidates,
        telemetry.prefix_cache_hits,
        telemetry.substring_cache_hits,
        telemetry.fuzzy_cache_hits,
        telemetry.prefix_cutoff_terms,
        telemetry.prefix_truncated_terms,
        telemetry.substring_term_truncated_grams + telemetry.substring_truncated_grams,
        telemetry.fuzzy_term_truncated_keys
            + telemetry.fuzzy_key_truncated_terms
            + telemetry.fuzzy_candidate_truncated_terms,
        metrics.violations,
        metrics.passed,
    )
    .map_err(|err| GfmError::io(path, err))
}

fn materialize_record_indexes(report: &MacrobenchReport) -> Result<u64> {
    let index_root = report.fixture_root.join("gate-indexes");
    if index_root.exists() {
        fs::remove_dir_all(&index_root).map_err(|err| GfmError::io(&index_root, err))?;
    }
    fs::create_dir_all(&index_root).map_err(|err| GfmError::io(&index_root, err))?;

    let mut total = 0;
    for scenario in MacrobenchScenario::ALL {
        let root = report.fixture_root.join(scenario.directory());
        let output = index_root.join(format!("{}.gfmidx", scenario.directory()));
        let snapshot = Indexer::default().build(&root)?;
        snapshot.save(&output)?;
        total += fs::metadata(&output)
            .map_err(|err| GfmError::io(&output, err))?
            .len();
    }
    Ok(total)
}

fn measure_sidecar_lookup(report: &MacrobenchReport) -> Result<SearchLookupTelemetry> {
    let mut telemetry = SearchLookupTelemetry::default();
    let sidecar_root = report.fixture_root.join("gate-sidecars");
    if sidecar_root.exists() {
        fs::remove_dir_all(&sidecar_root).map_err(|err| GfmError::io(&sidecar_root, err))?;
    }
    fs::create_dir_all(&sidecar_root).map_err(|err| GfmError::io(&sidecar_root, err))?;

    for scenario in MacrobenchScenario::ALL {
        let root = report.fixture_root.join(scenario.directory());
        let snapshot = Indexer::default().build(&root)?;
        let prefix_path = sidecar_root.join(format!("{}.gfmprefix", scenario.directory()));
        let substring_path = sidecar_root.join(format!("{}.gfmsubstr", scenario.directory()));
        let fuzzy_path = sidecar_root.join(format!("{}.gfmfuzzy", scenario.directory()));
        write_prefix_postings(
            &prefix_path,
            &prefix_postings_from_records(&snapshot.records),
        )?;
        write_substring_postings(
            &substring_path,
            &substring_postings_from_records(&snapshot.records),
        )?;
        write_fuzzy_postings(&fuzzy_path, &fuzzy_postings_from_records(&snapshot.records))?;
        let lookup = SearchArchiveLookup::open(&prefix_path, &substring_path, &fuzzy_path)?;
        let live = snapshot.into_live();
        for _ in 0..2 {
            let report = live.search_with_lookup_budget(
                "project",
                50,
                &lookup,
                SearchLookupBudget::default(),
            )?;
            telemetry.merge(&report.lookup);
        }
    }
    Ok(telemetry)
}

fn realistic_large_records(count: usize) -> Vec<FileRecord> {
    let mut records = Vec::with_capacity(count);
    for index in 0..count {
        let volume = match index % 6 {
            0 => VolumeId(1),
            1 => VolumeId(2),
            2 => VolumeId(3),
            3 => VolumeId(4),
            4 => VolumeId(5),
            _ => VolumeId(6),
        };
        let (path, name, kind, tags, comment) = realistic_record_shape(index);
        records.push(FileRecord {
            id: FileId::new(volume, index as u64 + 1),
            parent: None,
            path,
            name,
            kind,
            len: 1_024 + (index % 65_536) as u64,
            mode: 0o100644,
            owner: 501,
            group: 20,
            xattrs_digest: index as u64 ^ 0x9e37_79b9,
            created: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(index as u64)),
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(index as u64 * 3)),
            changed: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(index as u64 * 5)),
            hidden: index % 97 == 0,
            tags,
            finder_comment: comment,
        });
    }
    records
}

fn realistic_record_shape(
    index: usize,
) -> (PathBuf, String, FileKind, Vec<String>, Option<String>) {
    match index % 8 {
        0 => {
            let name = format!("project-source-{index:08}.rs");
            (
                PathBuf::from(format!(
                    "/Users/deepsaint/work/project-{}/src/{name}",
                    index % 4096
                )),
                name,
                FileKind::File,
                vec!["code".to_string()],
                Some("project implementation source".to_string()),
            )
        }
        1 => {
            let name = format!("Project Plan {index:08}.md");
            (
                PathBuf::from(format!(
                    "/Users/deepsaint/Documents/Plans/{}/{name}",
                    index % 512
                )),
                name,
                FileKind::File,
                vec!["important".to_string()],
                Some("planning document project notes".to_string()),
            )
        }
        2 => {
            let name = format!("IMG_{index:08}.heic");
            (
                PathBuf::from(format!(
                    "/Users/deepsaint/Pictures/Albums/{}/{name}",
                    index % 2048
                )),
                name,
                FileKind::File,
                vec!["media".to_string()],
                Some("media asset".to_string()),
            )
        }
        3 => {
            let name = format!("icloud-project-{index:08}.pages");
            (
                PathBuf::from(format!(
                    "/Users/deepsaint/Library/Mobile Documents/com~apple~CloudDocs/{name}"
                )),
                name,
                FileKind::File,
                vec!["icloud".to_string()],
                Some("icloud project placeholder".to_string()),
            )
        }
        4 => {
            let name = format!("external-project-{index:08}.mov");
            (
                PathBuf::from(format!(
                    "/Volumes/External Raid/Video/{}/{name}",
                    index % 256
                )),
                name,
                FileKind::File,
                vec!["external".to_string()],
                Some("external volume media project".to_string()),
            )
        }
        5 => {
            let name = format!("network-project-{index:08}.xlsx");
            (
                PathBuf::from(format!(
                    "/Volumes/Team Share/Reports/{}/{name}",
                    index % 256
                )),
                name,
                FileKind::File,
                vec!["network".to_string()],
                Some("network report project".to_string()),
            )
        }
        6 => {
            let name = format!("PackageProject{index:08}.app");
            (
                PathBuf::from(format!("/Applications/{name}")),
                name,
                FileKind::Directory,
                vec!["application".to_string()],
                Some("application bundle".to_string()),
            )
        }
        _ => {
            let name = format!("archive-project-{index:08}.zip");
            (
                PathBuf::from(format!(
                    "/Users/deepsaint/Downloads/Archives/{}/{name}",
                    index % 1024
                )),
                name,
                FileKind::File,
                vec!["archive".to_string()],
                Some("download archive project".to_string()),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MacrobenchMeasurement, MacrobenchScenario, MacrobenchStage};
    use gfm_telemetry::{
        BudgetViolation, FrameTiming, MemorySample, PerformanceBudgets, ScenarioMetric, Telemetry,
    };
    use std::path::PathBuf;
    use std::time::Duration;

    #[test]
    fn passes_when_inputs_are_within_gate() {
        let report = macrobench_report(Vec::new(), 100);
        let mut telemetry = Telemetry::default();
        telemetry.observe_memory(MemorySample {
            resident_bytes: 32 * 1024 * 1024,
            virtual_bytes: 64 * 1024 * 1024,
        });

        let gate = evaluate_regression_gate(
            &RegressionInputs {
                macrobench: &report,
                resources: Some(telemetry.resources()),
                frame_timing: None,
                index_size_bytes: Some(20_000),
                sidecar_lookup: Some(SearchLookupTelemetry {
                    prefix_candidate_ids: 128,
                    fuzzy_verified_candidates: 64,
                    ..SearchLookupTelemetry::default()
                }),
            },
            RegressionGateOptions::default(),
        );

        assert!(gate.passed());
    }

    #[test]
    fn fails_on_latency_memory_index_and_frame_drift() {
        let latency = BudgetViolation::ScenarioExceeded {
            metric: ScenarioMetric::ColdStart,
            observed: Duration::from_secs(2),
            budget: PerformanceBudgets::default()
                .scenario(ScenarioMetric::ColdStart)
                .unwrap()
                .max,
        };
        let report = macrobench_report(vec![latency], 10);
        let mut telemetry = Telemetry::default();
        telemetry.observe_memory(MemorySample {
            resident_bytes: 1024,
            virtual_bytes: 2048,
        });
        let mut frames = FrameTiming::with_stall_threshold(Duration::from_millis(16));
        frames.observe(Duration::from_millis(20));

        let gate = evaluate_regression_gate(
            &RegressionInputs {
                macrobench: &report,
                resources: Some(telemetry.resources()),
                frame_timing: Some(frames.summary()),
                index_size_bytes: Some(20_000),
                sidecar_lookup: Some(SearchLookupTelemetry {
                    prefix_candidate_ids: 999,
                    fuzzy_verified_candidates: 999,
                    prefix_truncated_terms: 1,
                    fuzzy_term_truncated_keys: 1,
                    fuzzy_key_truncated_terms: 1,
                    fuzzy_candidate_truncated_terms: 1,
                    ..SearchLookupTelemetry::default()
                }),
            },
            RegressionGateOptions {
                max_peak_memory_bytes: 512,
                max_index_bytes_per_record: 128,
                max_sidecar_prefix_candidate_ids: 128,
                max_sidecar_fuzzy_verified_candidates: 128,
                fail_on_sidecar_truncation: true,
                fail_on_frame_stalls: true,
            },
        );

        assert!(!gate.passed());
        assert!(gate
            .violations
            .iter()
            .any(|violation| matches!(violation, RegressionGateViolation::Latency(_))));
        assert!(gate.violations.iter().any(|violation| {
            matches!(
                violation,
                RegressionGateViolation::MemoryPeakExceeded { .. }
            )
        }));
        assert!(gate.violations.iter().any(|violation| {
            matches!(violation, RegressionGateViolation::IndexSizeExceeded { .. })
        }));
        assert!(gate.violations.iter().any(|violation| {
            matches!(
                violation,
                RegressionGateViolation::FrameStallDetected { .. }
            )
        }));
        assert!(gate.violations.iter().any(|violation| {
            matches!(
                violation,
                RegressionGateViolation::SidecarPrefixCandidatesExceeded { .. }
            )
        }));
        assert!(gate.violations.iter().any(|violation| {
            matches!(
                violation,
                RegressionGateViolation::SidecarFuzzyCandidatesExceeded { .. }
            )
        }));
        assert!(gate.violations.iter().any(|violation| {
            matches!(
                violation,
                RegressionGateViolation::SidecarLookupTruncated { .. }
            )
        }));
    }

    #[test]
    fn fails_on_sidecar_lookup_truncation_without_other_drift() {
        let report = macrobench_report(Vec::new(), 100);
        let gate = evaluate_regression_gate(
            &RegressionInputs {
                macrobench: &report,
                resources: None,
                frame_timing: None,
                index_size_bytes: None,
                sidecar_lookup: Some(SearchLookupTelemetry {
                    prefix_candidate_ids: 128,
                    fuzzy_verified_candidates: 64,
                    prefix_truncated_terms: 1,
                    ..SearchLookupTelemetry::default()
                }),
            },
            RegressionGateOptions::default(),
        );

        assert_eq!(
            gate.violations,
            vec![RegressionGateViolation::SidecarLookupTruncated {
                prefix_terms: 1,
                substring_terms: 0,
                fuzzy_terms_with_truncated_keys: 0,
                fuzzy_keys_with_truncated_terms: 0,
                fuzzy_candidate_terms: 0,
            }]
        );
    }

    #[test]
    fn run_regression_gate_materializes_index_size() {
        let root = unique_temp_dir("gfm-regression-gate");
        let run = run_regression_gate(
            &MacrobenchOptions::smoke(&root),
            RegressionGateOptions {
                max_peak_memory_bytes: u64::MAX,
                max_index_bytes_per_record: u64::MAX,
                max_sidecar_prefix_candidate_ids: usize::MAX,
                max_sidecar_fuzzy_verified_candidates: usize::MAX,
                fail_on_sidecar_truncation: false,
                fail_on_frame_stalls: true,
            },
        )
        .unwrap();

        assert!(run.index_size_bytes > 0);
        assert!(run.sidecar_lookup.prefix_terms > 0);
        assert!(run.sidecar_lookup.prefix_candidate_ids > 0);
        assert!(run.sidecar_lookup.prefix_cache_misses > 0);
        assert!(run.sidecar_lookup.prefix_cache_hits > 0);
        assert!(run.sidecar_lookup.fuzzy_cache_misses > 0);
        assert!(run.sidecar_lookup.fuzzy_cache_hits > 0);
        assert!(run
            .macrobench
            .fixture_root
            .join("gate-indexes")
            .join("small.gfmidx")
            .exists());
        assert!(run.passed());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn large_sidecar_gate_materializes_realistic_records_and_lookup_sidecars() {
        let root = unique_temp_dir("gfm-large-sidecar-gate");
        let report = run_large_sidecar_gate(&LargeSidecarGateOptions::new(&root, 4_096)).unwrap();

        assert_eq!(report.records, 4_096);
        assert_eq!(report.probe_records, 4_096);
        assert!(report.prefix_keys > 0);
        assert!(report.substring_keys > 0);
        assert!(report.fuzzy_keys > 0);
        assert!(report.prefix_bytes > 0);
        assert!(report.substring_bytes > 0);
        assert!(report.fuzzy_bytes > 0);
        assert!(report.lookup.prefix_terms > 0);
        assert!(report.lookup.substring_terms > 0);
        assert!(report.lookup.prefix_cache_misses > 0);
        assert!(report.lookup.prefix_cache_hits > 0);
        assert!(report.lookup.substring_cache_misses > 0);
        assert!(report.lookup.substring_cache_hits > 0);
        assert!(report.passed);
        assert!(report.violations.is_empty());
        assert_eq!(
            report.thresholds.profile,
            LargeSidecarThresholds::production_macos_million().profile
        );
        assert!(report.thresholds_path.exists());
        assert!(report.history_path.exists());
        assert!(fs::read_to_string(&report.thresholds_path)
            .unwrap()
            .contains("large-sidecar-thresholds\tprofile=production-macos-million-v1"));
        assert!(fs::read_to_string(&report.history_path)
            .unwrap()
            .contains("large-sidecar-history\trun=1"));
        assert!(report.fixture_root.join("records.gfmprefix").exists());
        assert!(report.fixture_root.join("records.gfmsubstr").exists());
        assert!(report.fixture_root.join("records.gfmfuzzy").exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn large_sidecar_gate_retains_history_across_runs() {
        let root = unique_temp_dir("gfm-large-sidecar-gate-history");
        let first = run_large_sidecar_gate(&LargeSidecarGateOptions::new(&root, 512)).unwrap();
        let second = run_large_sidecar_gate(&LargeSidecarGateOptions::new(&root, 512)).unwrap();

        assert_eq!(second.probe_records, 512);
        assert_eq!(first.history_path, second.history_path);
        let history = fs::read_to_string(&second.history_path).unwrap();
        assert_eq!(history.matches("large-sidecar-thresholds\t").count(), 1);
        assert!(history.contains("large-sidecar-history\trun=1"));
        assert!(history.contains("large-sidecar-history\trun=2"));

        fs::remove_dir_all(root).unwrap();
    }

    fn macrobench_report(
        budget_violations: Vec<BudgetViolation>,
        records: usize,
    ) -> MacrobenchReport {
        MacrobenchReport {
            fixture_root: PathBuf::from("/tmp/gfm-test"),
            files_materialized: records,
            measurements: vec![MacrobenchMeasurement {
                scenario: MacrobenchScenario::Small,
                stage: MacrobenchStage::IndexBuild,
                duration: Duration::from_millis(1),
                records,
                hits: 0,
            }],
            budget_violations,
        }
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
