mod gates;
mod macrobench;
mod parity;
mod pixel;

pub use gates::{
    evaluate_regression_gate, run_regression_gate, RegressionGateOptions, RegressionGateReport,
    RegressionGateRun, RegressionGateViolation, RegressionInputs,
};
pub use macrobench::{
    materialize_macrobench_fixture, run_macrobench, MacrobenchMeasurement, MacrobenchOptions,
    MacrobenchReport, MacrobenchScale, MacrobenchScenario, MacrobenchStage,
};
pub use parity::{
    materialize_parity_fixture, ParityFixtureOptions, ParityFixtureReport, ParityFixtureScale,
    ParityFixtureScenario, ParityFixtureScenarioReport,
};
pub use pixel::{
    diff_rgba, diff_rgba_files, evaluate_pixel_threshold, parse_masks, read_mask_file,
    ParitySurface, PixelDiffOptions, PixelDiffReport, PixelDriftThreshold, PixelMaskRect,
    PixelMismatch, PixelSize, PixelThresholdEvaluation, PixelThresholdViolation, ThresholdTsv,
};
