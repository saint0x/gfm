use crate::{
    run_macrobench, MacrobenchOptions, MacrobenchReport, MacrobenchScenario, MacrobenchStage,
};
use gfm_index::Indexer;
use gfm_telemetry::{BudgetViolation, FrameTimingSummary, ResourceSummary};
use gfm_types::{GfmError, Result};
use std::fs;

#[derive(Debug, Clone, PartialEq)]
pub struct RegressionInputs<'a> {
    pub macrobench: &'a MacrobenchReport,
    pub resources: Option<ResourceSummary>,
    pub frame_timing: Option<FrameTimingSummary>,
    pub index_size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegressionGateOptions {
    pub max_peak_memory_bytes: u64,
    pub max_index_bytes_per_record: u64,
    pub fail_on_frame_stalls: bool,
}

impl Default for RegressionGateOptions {
    fn default() -> Self {
        Self {
            max_peak_memory_bytes: 512 * 1024 * 1024,
            max_index_bytes_per_record: 512,
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegressionGateRun {
    pub macrobench: MacrobenchReport,
    pub gate: RegressionGateReport,
    pub index_size_bytes: u64,
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
    let gate = evaluate_regression_gate(
        &RegressionInputs {
            macrobench: &macrobench,
            resources: None,
            frame_timing: None,
            index_size_bytes: Some(index_size_bytes),
        },
        gate_options,
    );
    Ok(RegressionGateRun {
        macrobench,
        gate,
        index_size_bytes,
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
            },
            RegressionGateOptions {
                max_peak_memory_bytes: 512,
                max_index_bytes_per_record: 128,
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
    }

    #[test]
    fn run_regression_gate_materializes_index_size() {
        let root = unique_temp_dir("gfm-regression-gate");
        let run = run_regression_gate(
            &MacrobenchOptions::smoke(&root),
            RegressionGateOptions {
                max_peak_memory_bytes: u64::MAX,
                max_index_bytes_per_record: u64::MAX,
                fail_on_frame_stalls: true,
            },
        )
        .unwrap();

        assert!(run.index_size_bytes > 0);
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
