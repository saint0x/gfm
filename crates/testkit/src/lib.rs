mod gates;
mod macrobench;
mod parity;
mod parity_gate;
mod parity_profile;
mod pixel;

pub use gates::{
    evaluate_regression_gate, run_large_sidecar_gate, run_regression_gate, LargeSidecarGateOptions,
    LargeSidecarGateReport, RegressionGateOptions, RegressionGateReport, RegressionGateRun,
    RegressionGateViolation, RegressionInputs,
};
pub use macrobench::{
    materialize_macrobench_fixture, run_macrobench, MacrobenchMeasurement, MacrobenchOptions,
    MacrobenchReport, MacrobenchScale, MacrobenchScenario, MacrobenchStage,
};
pub use parity::{
    materialize_parity_fixture, ParityFixtureOptions, ParityFixtureReport, ParityFixtureScale,
    ParityFixtureScenario, ParityFixtureScenarioReport,
};
pub use parity_gate::{
    parse_parity_gate_manifest, run_parity_gate, run_parity_gate_manifest,
    write_parity_review_bundle, write_parity_review_bundle_manifest, ParityGateEntryReport,
    ParityGateInput, ParityGateReport, ParityReviewBundle,
};
pub use parity_profile::{
    ColorProfile, DimensionToken, DisplayScale, MacOsParityProfile, MaterialToken,
    ParityAppearance, SymbolToken, TimingToken, TypographyToken,
};
pub use pixel::{
    diff_rgba, diff_rgba_files, evaluate_pixel_threshold, parse_masks, read_mask_file,
    ParitySurface, PixelDiffOptions, PixelDiffReport, PixelDriftThreshold, PixelMaskRect,
    PixelMismatch, PixelSize, PixelThresholdEvaluation, PixelThresholdViolation, ThresholdTsv,
};
