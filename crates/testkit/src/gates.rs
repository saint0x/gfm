use crate::{
    diff_rgba_files, evaluate_pixel_threshold, read_mask_file, run_macrobench, MacrobenchOptions,
    MacrobenchReport, MacrobenchScenario, MacrobenchStage, ParitySurface, PixelDiffOptions,
    PixelDiffReport, PixelDriftThreshold, PixelSize, PixelThresholdEvaluation,
};
use gfm_index::Indexer;
use gfm_telemetry::{BudgetViolation, FrameTimingSummary, ResourceSummary};
use gfm_types::{GfmError, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParityGateInput {
    pub surface: ParitySurface,
    pub expected_path: PathBuf,
    pub actual_path: PathBuf,
    pub size: PixelSize,
    pub mask_path: Option<PathBuf>,
}

impl ParityGateInput {
    pub fn new(
        surface: ParitySurface,
        expected_path: impl Into<PathBuf>,
        actual_path: impl Into<PathBuf>,
        size: PixelSize,
    ) -> Self {
        Self {
            surface,
            expected_path: expected_path.into(),
            actual_path: actual_path.into(),
            size,
            mask_path: None,
        }
    }

    pub fn with_mask(mut self, mask_path: impl Into<PathBuf>) -> Self {
        self.mask_path = Some(mask_path.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParityGateReport {
    pub manifest_path: Option<PathBuf>,
    pub entries: Vec<ParityGateEntryReport>,
}

impl ParityGateReport {
    pub fn passed(&self) -> bool {
        self.entries.iter().all(ParityGateEntryReport::passed)
    }

    pub fn violations(&self) -> usize {
        self.entries
            .iter()
            .map(|entry| entry.evaluation.violations.len())
            .sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParityGateEntryReport {
    pub input: ParityGateInput,
    pub diff: PixelDiffReport,
    pub evaluation: PixelThresholdEvaluation,
}

impl ParityGateEntryReport {
    pub fn passed(&self) -> bool {
        self.evaluation.passed
    }
}

pub fn run_parity_gate_manifest(path: impl AsRef<Path>) -> Result<ParityGateReport> {
    let path = path.as_ref();
    let content = fs::read_to_string(path).map_err(|err| GfmError::io(path, err))?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let inputs = parse_parity_gate_manifest(&content, base)?;
    let mut report = run_parity_gate(inputs)?;
    report.manifest_path = Some(path.to_path_buf());
    Ok(report)
}

pub fn run_parity_gate(inputs: Vec<ParityGateInput>) -> Result<ParityGateReport> {
    let mut entries = Vec::with_capacity(inputs.len());
    for input in inputs {
        let masks = input
            .mask_path
            .as_ref()
            .map(|path| read_mask_file(path, input.size))
            .transpose()?
            .unwrap_or_default();
        let options = PixelDiffOptions::strict(input.size).with_masks(masks);
        let diff = diff_rgba_files(&input.expected_path, &input.actual_path, &options)?;
        let threshold = PixelDriftThreshold::finder_strict(input.surface);
        let evaluation = evaluate_pixel_threshold(&diff, threshold);
        entries.push(ParityGateEntryReport {
            input,
            diff,
            evaluation,
        });
    }
    Ok(ParityGateReport {
        manifest_path: None,
        entries,
    })
}

pub fn parse_parity_gate_manifest(content: &str, base: &Path) -> Result<Vec<ParityGateInput>> {
    let mut inputs = Vec::new();
    for (line_index, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 5 && fields.len() != 6 {
            return Err(GfmError::Format(format!(
                "parity gate manifest line {} must contain surface, expected, actual, width, height, and optional mask",
                line_index + 1
            )));
        }
        let surface = ParitySurface::from_str(fields[0]).map_err(GfmError::Format)?;
        let expected_path = resolve_manifest_path(base, fields[1]);
        let actual_path = resolve_manifest_path(base, fields[2]);
        let width = parse_manifest_u32(line_index, "width", fields[3])?;
        let height = parse_manifest_u32(line_index, "height", fields[4])?;
        let mut input = ParityGateInput::new(
            surface,
            expected_path,
            actual_path,
            PixelSize::new(width, height),
        );
        if fields.len() == 6 && !fields[5].is_empty() {
            input = input.with_mask(resolve_manifest_path(base, fields[5]));
        }
        inputs.push(input);
    }
    if inputs.is_empty() {
        return Err(GfmError::Format(
            "parity gate manifest does not contain any entries".to_string(),
        ));
    }
    Ok(inputs)
}

fn resolve_manifest_path(base: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn parse_manifest_u32(line_index: usize, name: &str, value: &str) -> Result<u32> {
    value.parse::<u32>().map_err(|_| {
        GfmError::Format(format!(
            "parity gate manifest line {} has invalid {name}: {value}",
            line_index + 1
        ))
    })
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

    #[test]
    fn parity_gate_passes_only_explicitly_masked_drift() {
        let root = unique_temp_dir("gfm-parity-gate");
        let expected = root.join("expected.rgba");
        let actual = root.join("actual.rgba");
        let mask = root.join("mask.tsv");
        fs::write(&expected, [0, 0, 0, 255, 10, 10, 10, 255]).unwrap();
        fs::write(&actual, [0, 0, 0, 255, 9, 10, 10, 255]).unwrap();
        fs::write(&mask, "1\t0\t1\t1\n").unwrap();

        let report = run_parity_gate(vec![ParityGateInput::new(
            ParitySurface::Toolbar,
            &expected,
            &actual,
            PixelSize::new(2, 1),
        )
        .with_mask(&mask)])
        .unwrap();

        assert!(report.passed());
        assert_eq!(report.violations(), 0);
        assert_eq!(report.entries[0].diff.masked_mismatches, 1);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parity_gate_fails_unapproved_drift() {
        let root = unique_temp_dir("gfm-parity-gate-fail");
        let expected = root.join("expected.rgba");
        let actual = root.join("actual.rgba");
        fs::write(&expected, [0, 0, 0, 255, 10, 10, 10, 255]).unwrap();
        fs::write(&actual, [0, 0, 0, 255, 9, 10, 10, 255]).unwrap();

        let report = run_parity_gate(vec![ParityGateInput::new(
            ParitySurface::Text,
            &expected,
            &actual,
            PixelSize::new(2, 1),
        )])
        .unwrap();

        assert!(!report.passed());
        assert_eq!(report.violations(), 1);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parity_gate_manifest_resolves_relative_artifacts() {
        let root = unique_temp_dir("gfm-parity-gate-manifest");
        fs::write(root.join("expected.rgba"), [1, 2, 3, 255]).unwrap();
        fs::write(root.join("actual.rgba"), [1, 2, 3, 255]).unwrap();
        fs::write(
            root.join("gate.tsv"),
            "icon\texpected.rgba\tactual.rgba\t1\t1\n",
        )
        .unwrap();

        let report = run_parity_gate_manifest(root.join("gate.tsv")).unwrap();

        assert!(report.passed());
        assert_eq!(report.entries.len(), 1);
        assert!(report.entries[0]
            .input
            .expected_path
            .ends_with("expected.rgba"));

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
