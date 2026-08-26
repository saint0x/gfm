mod gates;
mod macrobench;
mod parity;
mod parity_gate;
mod parity_profile;
mod pixel;

pub use gates::{
    evaluate_regression_gate, run_large_sidecar_gate, run_regression_gate,
    run_search_typing_benchmark, LargeSidecarGateOptions, LargeSidecarGateReport,
    LargeSidecarGateViolation, LargeSidecarThresholds, RegressionGateOptions, RegressionGateReport,
    RegressionGateRun, RegressionGateViolation, RegressionInputs, SearchTypingBenchmarkOptions,
    SearchTypingBenchmarkReport, SearchTypingBenchmarkViolation,
};
pub use macrobench::{
    materialize_macrobench_fixture, materialize_macrobench_fixture_report, run_macrobench,
    MacrobenchFixtureReport, MacrobenchFixtureScenarioReport, MacrobenchMeasurement,
    MacrobenchOptions, MacrobenchReport, MacrobenchScale, MacrobenchScenario, MacrobenchStage,
};
pub use parity::{
    materialize_parity_fixture, ParityFixtureOptions, ParityFixtureReport, ParityFixtureScale,
    ParityFixtureScenario, ParityFixtureScenarioReport,
};
pub use parity_gate::{
    parse_parity_gate_manifest, run_parity_gate, run_parity_gate_manifest,
    write_parity_review_bundle, write_parity_review_bundle_manifest, ParityCaptureProvenance,
    ParityFocusState, ParityGateEntryReport, ParityGateInput, ParityGateReport, ParityReviewBundle,
    ParityViewMode,
};
pub use parity_profile::{
    ColorProfile, DimensionToken, DisplayScale, MacOsParityProfile, MaterialToken,
    ParityAppearance, SymbolToken, TimingToken, TypographyToken,
};
pub use pixel::{
    diff_image_files, diff_rgba, diff_rgba_files, evaluate_pixel_threshold, parse_governed_masks,
    parse_masks, read_governed_mask_file, read_mask_file, read_rgba_image_file,
    write_visual_diff_png, ParitySurface, PixelDiffOptions, PixelDiffReport, PixelDriftThreshold,
    PixelMaskRect, PixelMaskRegion, PixelMismatch, PixelRegionSummary, PixelSize,
    PixelThresholdEvaluation, PixelThresholdViolation, RgbaImage, ThresholdTsv,
};
