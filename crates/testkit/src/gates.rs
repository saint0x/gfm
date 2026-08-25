use crate::{
    run_macrobench, MacrobenchOptions, MacrobenchReport, MacrobenchScenario, MacrobenchStage,
};
use gfm_index::{Indexer, SearchArchiveLookup, SearchLookupBudget, SearchLookupTelemetry};
use gfm_store::{
    fuzzy_postings_from_records, prefix_postings_from_records, write_fuzzy_postings,
    write_prefix_postings,
};
use gfm_telemetry::{BudgetViolation, FrameTimingSummary, ResourceSummary};
use gfm_types::{GfmError, Result};
use std::fs;

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
        if options.fail_on_sidecar_truncation
            && (sidecar.prefix_truncated_terms > 0
                || sidecar.fuzzy_term_truncated_keys > 0
                || sidecar.fuzzy_key_truncated_terms > 0
                || sidecar.fuzzy_candidate_truncated_terms > 0)
        {
            violations.push(RegressionGateViolation::SidecarLookupTruncated {
                prefix_terms: sidecar.prefix_truncated_terms,
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
        let fuzzy_path = sidecar_root.join(format!("{}.gfmfuzzy", scenario.directory()));
        write_prefix_postings(
            &prefix_path,
            &prefix_postings_from_records(&snapshot.records),
        )?;
        write_fuzzy_postings(&fuzzy_path, &fuzzy_postings_from_records(&snapshot.records))?;
        let lookup = SearchArchiveLookup::open(&prefix_path, &fuzzy_path)?;
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
