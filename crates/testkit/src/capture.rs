use crate::{
    run_parity_gate_manifest, write_parity_review_bundle, ColorProfile, DisplayScale,
    ParityAppearance, ParityFocusState, ParityReviewBundle, ParitySurface, ParityViewMode,
    PixelSize,
};
use gfm_types::{GfmError, Result};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParityCaptureTarget {
    Finder,
    Gfm,
}

impl ParityCaptureTarget {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Finder => "finder",
            Self::Gfm => "gfm",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "finder" => Ok(Self::Finder),
            "gfm" => Ok(Self::Gfm),
            other => Err(GfmError::Format(format!(
                "parity capture target must be finder or gfm; got `{other}`"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParityScreenshotCaptureOptions {
    pub target: ParityCaptureTarget,
    pub fixture_root: PathBuf,
    pub output_png: PathBuf,
    pub provenance_tsv: PathBuf,
    pub scenario: String,
    pub view_mode: ParityViewMode,
    pub macos_build: String,
    pub hardware_profile: String,
    pub display_profile: String,
    pub app_version: String,
    pub captured_at: String,
    pub expires_at: Option<String>,
    pub reviewer: String,
    pub signer: String,
    pub approved_mask_set: String,
    pub appearance: ParityAppearance,
    pub scale: DisplayScale,
    pub color_profile: ColorProfile,
    pub focus: ParityFocusState,
    pub window_origin_x: u32,
    pub window_origin_y: u32,
    pub window_size: PixelSize,
    pub gfm_app: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParityScreenshotCaptureReport {
    pub target: ParityCaptureTarget,
    pub fixture_root: PathBuf,
    pub output_png: PathBuf,
    pub provenance_tsv: PathBuf,
    pub window_region: CaptureRegion,
    pub prepare_command: Vec<String>,
    pub capture_command: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParityCapturePairManifestOptions {
    pub manifest_path: PathBuf,
    pub surface: ParitySurface,
    pub finder: ParityScreenshotCaptureOptions,
    pub gfm: ParityScreenshotCaptureOptions,
    pub mask_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParityCaptureMatrixOptions {
    pub plan_path: PathBuf,
    pub fixture_root: PathBuf,
    pub artifact_root: PathBuf,
    pub macos_build: String,
    pub hardware_profile: String,
    pub display_profile: String,
    pub app_version: String,
    pub captured_at: String,
    pub expires_at: Option<String>,
    pub reviewer: String,
    pub signer: String,
    pub approved_mask_set: String,
    pub appearance: ParityAppearance,
    pub scale: DisplayScale,
    pub color_profile: ColorProfile,
    pub focus: ParityFocusState,
    pub window_origin_x: u32,
    pub window_origin_y: u32,
    pub window_size: PixelSize,
    pub gfm_app: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParityCaptureMatrixRow {
    pub surface: ParitySurface,
    pub scenario: String,
    pub view_mode: ParityViewMode,
    pub finder_output: PathBuf,
    pub finder_provenance: PathBuf,
    pub gfm_output: PathBuf,
    pub gfm_provenance: PathBuf,
    pub mask_path: PathBuf,
    pub manifest_path: PathBuf,
    pub finder_command: Vec<String>,
    pub gfm_command: Vec<String>,
    pub manifest_command: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParityCaptureMatrixReport {
    pub plan_path: PathBuf,
    pub rows: Vec<ParityCaptureMatrixRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParityCaptureMatrixExecutionOptions {
    pub matrix: ParityCaptureMatrixOptions,
    pub review_root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParityCaptureMatrixExecutionRow {
    pub surface: ParitySurface,
    pub scenario: String,
    pub finder_output: PathBuf,
    pub gfm_output: PathBuf,
    pub manifest_path: PathBuf,
    pub review_dir: PathBuf,
    pub passed: bool,
    pub violations: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParityCaptureMatrixExecutionReport {
    pub plan_path: PathBuf,
    pub review_root: PathBuf,
    pub rows: Vec<ParityCaptureMatrixExecutionRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl CaptureRegion {
    fn screencapture_region(self) -> String {
        format!("{},{},{},{}", self.x, self.y, self.width, self.height)
    }
}

pub fn capture_parity_screenshot(
    options: &ParityScreenshotCaptureOptions,
) -> Result<ParityScreenshotCaptureReport> {
    capture_parity_screenshot_checked(options, || Ok(()))
}

pub fn capture_parity_screenshot_checked(
    options: &ParityScreenshotCaptureOptions,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<ParityScreenshotCaptureReport> {
    check_control()?;
    validate_capture_options(options)?;
    check_control()?;
    if let Some(parent) = options.output_png.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    if let Some(parent) = options.provenance_tsv.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }

    let region = CaptureRegion {
        x: options.window_origin_x,
        y: options.window_origin_y,
        width: options.window_size.width,
        height: options.window_size.height,
    };
    let prepare_command = prepare_capture_command(options)?;
    let capture_command = screencapture_command(&region, &options.output_png);

    validate_capture_host_paths(options)?;
    run_command(&prepare_command, "parity capture prepare")?;
    check_control()?;
    run_command(&capture_command, "parity screenshot capture")?;
    check_control()?;
    write_capture_provenance(options, &region)?;

    Ok(ParityScreenshotCaptureReport {
        target: options.target,
        fixture_root: options.fixture_root.clone(),
        output_png: options.output_png.clone(),
        provenance_tsv: options.provenance_tsv.clone(),
        window_region: region,
        prepare_command,
        capture_command,
    })
}

pub fn plan_parity_capture_commands(
    options: &ParityScreenshotCaptureOptions,
) -> Result<(Vec<String>, Vec<String>)> {
    validate_capture_options(options)?;
    let region = CaptureRegion {
        x: options.window_origin_x,
        y: options.window_origin_y,
        width: options.window_size.width,
        height: options.window_size.height,
    };
    Ok((
        prepare_capture_command(options)?,
        screencapture_command(&region, &options.output_png),
    ))
}

pub fn write_parity_capture_pair_manifest(
    options: &ParityCapturePairManifestOptions,
) -> Result<()> {
    write_parity_capture_pair_manifest_checked(options, || Ok(()))
}

pub fn write_parity_capture_pair_manifest_checked(
    options: &ParityCapturePairManifestOptions,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<()> {
    check_control()?;
    validate_capture_pair_manifest_options(options)?;
    check_control()?;
    if let Some(parent) = options.manifest_path.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    let finder = &options.finder;
    let gfm = &options.gfm;
    let mask = options
        .mask_path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let expires_at = finder
        .expires_at
        .as_deref()
        .map(|value| format!("\texpires-at={}", escape_tsv_field(value)))
        .unwrap_or_default();
    let content = format!(
        "manifest-version\t1\nprofile\tmacos-build={}\thardware-profile={}\tdisplay-profile={}\tapp-version={}\tfixture-manifest={}\tcaptured-at={}{}\tcapture-command={}\treviewer={}\tsigner={}\tapproved-mask-set={}\tappearance={}\tscale={}\tcolor-profile={}\nentry\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        escape_tsv_field(&finder.macos_build),
        escape_tsv_field(&finder.hardware_profile),
        escape_tsv_field(&finder.display_profile),
        escape_tsv_field(&finder.app_version),
        escape_tsv_field(&capture_fixture_manifest_path(&finder.fixture_root).to_string_lossy()),
        escape_tsv_field(&finder.captured_at),
        expires_at,
        escape_tsv_field(&format!(
            "finder:screencapture:-x:-R:{},{},{},{};gfm:screencapture:-x:-R:{},{},{},{}",
            finder.window_origin_x,
            finder.window_origin_y,
            finder.window_size.width,
            finder.window_size.height,
            gfm.window_origin_x,
            gfm.window_origin_y,
            gfm.window_size.width,
            gfm.window_size.height
        )),
        escape_tsv_field(&finder.reviewer),
        escape_tsv_field(&finder.signer),
        escape_tsv_field(&finder.approved_mask_set),
        finder.appearance.as_str(),
        finder.scale.as_str(),
        finder.color_profile.as_str(),
        options.surface.as_str(),
        escape_tsv_field(&finder.output_png.to_string_lossy()),
        escape_tsv_field(&gfm.output_png.to_string_lossy()),
        finder.window_size.width,
        finder.window_size.height,
        escape_tsv_field(&mask),
        finder.window_size.width,
        finder.window_size.height,
        finder.focus.as_str(),
        finder.view_mode.as_str(),
        escape_tsv_field(&finder.fixture_root.to_string_lossy())
    );
    fs::write(&options.manifest_path, content)
        .map_err(|err| GfmError::io(&options.manifest_path, err))
}

fn capture_fixture_manifest_path(fixture_root: &Path) -> PathBuf {
    let direct = fixture_root.join("manifest.tsv");
    if direct.is_file() {
        return direct;
    }
    fixture_root
        .parent()
        .map(|parent| parent.join("manifest.tsv"))
        .unwrap_or(direct)
}

pub fn write_parity_capture_matrix_plan(
    options: &ParityCaptureMatrixOptions,
) -> Result<ParityCaptureMatrixReport> {
    write_parity_capture_matrix_plan_checked(options, || Ok(()))
}

pub fn write_parity_capture_matrix_plan_checked(
    options: &ParityCaptureMatrixOptions,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<ParityCaptureMatrixReport> {
    check_control()?;
    validate_capture_matrix_options(options)?;
    check_control()?;
    if let Some(parent) = options.plan_path.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    fs::create_dir_all(&options.artifact_root)
        .map_err(|err| GfmError::io(&options.artifact_root, err))?;
    let rows = build_capture_matrix_rows(options)?;
    check_control()?;
    fs::write(&options.plan_path, render_capture_matrix_plan(&rows))
        .map_err(|err| GfmError::io(&options.plan_path, err))?;
    Ok(ParityCaptureMatrixReport {
        plan_path: options.plan_path.clone(),
        rows,
    })
}

pub fn execute_parity_capture_matrix(
    options: &ParityCaptureMatrixExecutionOptions,
) -> Result<ParityCaptureMatrixExecutionReport> {
    execute_parity_capture_matrix_checked(options, || Ok(()))
}

pub fn execute_parity_capture_matrix_checked(
    options: &ParityCaptureMatrixExecutionOptions,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<ParityCaptureMatrixExecutionReport> {
    execute_parity_capture_matrix_with_capture(options, &mut check_control, |capture, check| {
        capture_parity_screenshot_checked(capture, check)
    })
}

fn execute_parity_capture_matrix_with_capture(
    options: &ParityCaptureMatrixExecutionOptions,
    check_control: &mut impl FnMut() -> Result<()>,
    mut capture: impl FnMut(
        &ParityScreenshotCaptureOptions,
        &mut dyn FnMut() -> Result<()>,
    ) -> Result<ParityScreenshotCaptureReport>,
) -> Result<ParityCaptureMatrixExecutionReport> {
    check_control()?;
    validate_capture_matrix_execution_options(options)?;
    check_control()?;
    fs::create_dir_all(&options.review_root)
        .map_err(|err| GfmError::io(&options.review_root, err))?;
    let plan = write_parity_capture_matrix_plan_checked(&options.matrix, &mut *check_control)?;
    let mut rows = Vec::with_capacity(plan.rows.len());
    for row in &plan.rows {
        let finder = matrix_capture_options(
            &options.matrix,
            ParityCaptureTarget::Finder,
            &row.scenario,
            row.view_mode,
            row.finder_output.clone(),
            row.finder_provenance.clone(),
        );
        let gfm = matrix_capture_options(
            &options.matrix,
            ParityCaptureTarget::Gfm,
            &row.scenario,
            row.view_mode,
            row.gfm_output.clone(),
            row.gfm_provenance.clone(),
        );
        capture(&finder, check_control)?;
        check_control()?;
        capture(&gfm, check_control)?;
        check_control()?;
        let mask_path = row.mask_path.is_file().then(|| row.mask_path.clone());
        write_parity_capture_pair_manifest_checked(
            &ParityCapturePairManifestOptions {
                manifest_path: row.manifest_path.clone(),
                surface: row.surface,
                finder,
                gfm,
                mask_path,
            },
            &mut *check_control,
        )?;
        check_control()?;
        let mut gate = run_parity_gate_manifest(&row.manifest_path)?;
        gate.manifest_path = Some(row.manifest_path.clone());
        let review_dir = options.review_root.join(row.surface.as_str());
        let bundle = write_parity_review_bundle(gate, &review_dir)?;
        rows.push(execution_row_from_bundle(row, &bundle));
        check_control()?;
    }
    Ok(ParityCaptureMatrixExecutionReport {
        plan_path: plan.plan_path,
        review_root: options.review_root.clone(),
        rows,
    })
}

fn execution_row_from_bundle(
    row: &ParityCaptureMatrixRow,
    bundle: &ParityReviewBundle,
) -> ParityCaptureMatrixExecutionRow {
    ParityCaptureMatrixExecutionRow {
        surface: row.surface,
        scenario: row.scenario.clone(),
        finder_output: row.finder_output.clone(),
        gfm_output: row.gfm_output.clone(),
        manifest_path: row.manifest_path.clone(),
        review_dir: bundle.output_dir.clone(),
        passed: bundle.report.passed(),
        violations: bundle.report.violations(),
    }
}

fn validate_capture_options(options: &ParityScreenshotCaptureOptions) -> Result<()> {
    validate_capture_metadata(options)?;
    if options.target == ParityCaptureTarget::Gfm && options.gfm_app.is_none() {
        return Err(GfmError::Format(
            "gfm parity capture requires a GFM app path".to_string(),
        ));
    }
    Ok(())
}

fn validate_capture_host_paths(options: &ParityScreenshotCaptureOptions) -> Result<()> {
    if let (ParityCaptureTarget::Gfm, Some(app)) = (options.target, options.gfm_app.as_ref()) {
        match fs::metadata(app) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(GfmError::Format(format!(
                    "gfm parity capture GFM app missing: {} is not an app bundle directory",
                    app.display()
                )));
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(GfmError::Format(format!(
                    "gfm parity capture GFM app missing: {}",
                    app.display()
                )));
            }
            Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
                return Err(GfmError::Format(format!(
                    "gfm parity capture GFM app denied: {}; {}",
                    app.display(),
                    err
                )));
            }
            Err(err) => {
                return Err(GfmError::Format(format!(
                    "gfm parity capture GFM app unavailable: {}; {}",
                    app.display(),
                    err
                )));
            }
        }
    }
    Ok(())
}

fn validate_capture_metadata(options: &ParityScreenshotCaptureOptions) -> Result<()> {
    if !options.fixture_root.is_dir() {
        return Err(GfmError::Format(format!(
            "parity capture fixture root must be a directory: {}",
            options.fixture_root.display()
        )));
    }
    if options.output_png.as_os_str().is_empty() {
        return Err(GfmError::Format(
            "parity capture output path cannot be empty".to_string(),
        ));
    }
    if options.provenance_tsv.as_os_str().is_empty() {
        return Err(GfmError::Format(
            "parity capture provenance path cannot be empty".to_string(),
        ));
    }
    if options.scenario.trim().is_empty()
        || options.macos_build.trim().is_empty()
        || options.hardware_profile.trim().is_empty()
        || options.display_profile.trim().is_empty()
        || options.app_version.trim().is_empty()
        || options.captured_at.trim().is_empty()
        || options.reviewer.trim().is_empty()
        || options.signer.trim().is_empty()
        || options.approved_mask_set.trim().is_empty()
    {
        return Err(GfmError::Format(
            "parity capture metadata fields cannot be empty".to_string(),
        ));
    }
    if options.window_size.width == 0 || options.window_size.height == 0 {
        return Err(GfmError::Format(
            "parity capture window size must be positive".to_string(),
        ));
    }
    if options.appearance == ParityAppearance::System {
        return Err(GfmError::Format(
            "parity capture appearance must be resolved light or dark".to_string(),
        ));
    }
    if !is_valid_utc_capture_timestamp(&options.captured_at) {
        return Err(GfmError::Format(format!(
            "parity capture captured-at must use UTC second precision: {}",
            options.captured_at
        )));
    }
    if let Some(expires_at) = &options.expires_at {
        if expires_at.trim().is_empty() {
            return Err(GfmError::Format(
                "parity capture expires-at cannot be empty".to_string(),
            ));
        }
        if !is_valid_utc_capture_timestamp(expires_at) {
            return Err(GfmError::Format(format!(
                "parity capture expires-at must use UTC second precision: {expires_at}"
            )));
        }
        if utc_capture_timestamp_epoch_seconds(&options.captured_at)
            > utc_capture_timestamp_epoch_seconds(expires_at)
        {
            return Err(GfmError::Format(format!(
                "parity capture captured-at `{}` cannot be after expires-at `{}`",
                options.captured_at, expires_at
            )));
        }
    }
    Ok(())
}

fn validate_capture_pair_manifest_options(
    options: &ParityCapturePairManifestOptions,
) -> Result<()> {
    validate_capture_metadata(&options.finder)?;
    validate_capture_metadata(&options.gfm)?;
    if options.finder.target != ParityCaptureTarget::Finder {
        return Err(GfmError::Format(
            "parity capture manifest requires a Finder capture as expected artifact".to_string(),
        ));
    }
    if options.gfm.target != ParityCaptureTarget::Gfm {
        return Err(GfmError::Format(
            "parity capture manifest requires a GFM capture as actual artifact".to_string(),
        ));
    }
    if options.finder.fixture_root != options.gfm.fixture_root
        || options.finder.scenario != options.gfm.scenario
        || options.finder.view_mode != options.gfm.view_mode
        || options.finder.macos_build != options.gfm.macos_build
        || options.finder.hardware_profile != options.gfm.hardware_profile
        || options.finder.display_profile != options.gfm.display_profile
        || options.finder.app_version != options.gfm.app_version
        || options.finder.captured_at != options.gfm.captured_at
        || options.finder.expires_at != options.gfm.expires_at
        || options.finder.reviewer != options.gfm.reviewer
        || options.finder.signer != options.gfm.signer
        || options.finder.approved_mask_set != options.gfm.approved_mask_set
        || options.finder.appearance != options.gfm.appearance
        || options.finder.scale != options.gfm.scale
        || options.finder.color_profile != options.gfm.color_profile
        || options.finder.focus != options.gfm.focus
        || options.finder.window_size != options.gfm.window_size
    {
        return Err(GfmError::Format(
            "parity capture manifest requires matching Finder and GFM capture metadata".to_string(),
        ));
    }
    if options.finder.output_png == options.gfm.output_png {
        return Err(GfmError::Format(
            "parity capture manifest requires distinct Finder and GFM screenshots".to_string(),
        ));
    }
    Ok(())
}

fn validate_capture_matrix_options(options: &ParityCaptureMatrixOptions) -> Result<()> {
    if !options.fixture_root.is_dir() {
        return Err(GfmError::Format(format!(
            "parity capture matrix fixture root must be a directory: {}",
            options.fixture_root.display()
        )));
    }
    if options.artifact_root.as_os_str().is_empty() || options.plan_path.as_os_str().is_empty() {
        return Err(GfmError::Format(
            "parity capture matrix paths cannot be empty".to_string(),
        ));
    }
    if options.gfm_app.as_os_str().is_empty() {
        return Err(GfmError::Format(
            "parity capture matrix requires a GFM app path".to_string(),
        ));
    }
    let probe = ParityScreenshotCaptureOptions {
        target: ParityCaptureTarget::Finder,
        fixture_root: options.fixture_root.clone(),
        output_png: options.artifact_root.join("probe.png"),
        provenance_tsv: options.artifact_root.join("probe.provenance.tsv"),
        scenario: "probe".to_string(),
        view_mode: ParityViewMode::Icon,
        macos_build: options.macos_build.clone(),
        hardware_profile: options.hardware_profile.clone(),
        display_profile: options.display_profile.clone(),
        app_version: options.app_version.clone(),
        captured_at: options.captured_at.clone(),
        expires_at: options.expires_at.clone(),
        reviewer: options.reviewer.clone(),
        signer: options.signer.clone(),
        approved_mask_set: options.approved_mask_set.clone(),
        appearance: options.appearance,
        scale: options.scale,
        color_profile: options.color_profile,
        focus: options.focus,
        window_origin_x: options.window_origin_x,
        window_origin_y: options.window_origin_y,
        window_size: options.window_size,
        gfm_app: None,
    };
    validate_capture_metadata(&probe)
}

fn validate_capture_matrix_execution_options(
    options: &ParityCaptureMatrixExecutionOptions,
) -> Result<()> {
    validate_capture_matrix_options(&options.matrix)?;
    if options.review_root.as_os_str().is_empty() {
        return Err(GfmError::Format(
            "parity capture matrix review root cannot be empty".to_string(),
        ));
    }
    Ok(())
}

fn build_capture_matrix_rows(
    options: &ParityCaptureMatrixOptions,
) -> Result<Vec<ParityCaptureMatrixRow>> {
    let mut rows = Vec::with_capacity(ParitySurface::ALL.len());
    for surface in ParitySurface::ALL {
        check_surface_capture_target(surface)?;
        let (scenario, view_mode) = capture_target_for_surface(surface);
        let stem = surface.as_str();
        let surface_root = options.artifact_root.join(stem);
        let finder_output = surface_root.join("finder.png");
        let finder_provenance = surface_root.join("finder.provenance.tsv");
        let gfm_output = surface_root.join("gfm.png");
        let gfm_provenance = surface_root.join("gfm.provenance.tsv");
        let mask_path = surface_root.join(format!(
            "mask-{}-{}.tsv",
            options.macos_build,
            options.appearance.as_str()
        ));
        let manifest_path = surface_root.join("gate.tsv");
        let finder = matrix_capture_options(
            options,
            ParityCaptureTarget::Finder,
            scenario,
            view_mode,
            finder_output.clone(),
            finder_provenance.clone(),
        );
        let gfm = matrix_capture_options(
            options,
            ParityCaptureTarget::Gfm,
            scenario,
            view_mode,
            gfm_output.clone(),
            gfm_provenance.clone(),
        );
        plan_parity_capture_commands(&finder)?;
        plan_parity_capture_commands(&gfm)?;
        rows.push(ParityCaptureMatrixRow {
            surface,
            scenario: scenario.to_string(),
            view_mode,
            finder_output,
            finder_provenance,
            gfm_output,
            gfm_provenance,
            mask_path: mask_path.clone(),
            manifest_path: manifest_path.clone(),
            finder_command: parity_capture_cli_command(&finder),
            gfm_command: parity_capture_cli_command(&gfm),
            manifest_command: parity_capture_manifest_cli_command(&MatrixManifestCommandInput {
                options,
                surface,
                scenario,
                view_mode,
                manifest_path: &manifest_path,
                mask_path: &mask_path,
                finder_output: &finder.output_png,
                gfm_output: &gfm.output_png,
            }),
        });
    }
    Ok(rows)
}

fn check_surface_capture_target(surface: ParitySurface) -> Result<()> {
    let (scenario, view_mode) = capture_target_for_surface(surface);
    if scenario.is_empty() {
        return Err(GfmError::Format(format!(
            "parity capture matrix missing scenario for {}",
            surface.as_str()
        )));
    }
    if view_mode.as_str().is_empty() {
        return Err(GfmError::Format(format!(
            "parity capture matrix missing view mode for {}",
            surface.as_str()
        )));
    }
    Ok(())
}

fn capture_target_for_surface(surface: ParitySurface) -> (&'static str, ParityViewMode) {
    match surface {
        ParitySurface::Layout
        | ParitySurface::Icon
        | ParitySurface::Focus
        | ParitySurface::Hover => ("icon", ParityViewMode::Icon),
        ParitySurface::Text => ("list", ParityViewMode::List),
        ParitySurface::Sidebar => ("sidebar", ParityViewMode::Column),
        ParitySurface::Selection => ("selection", ParityViewMode::Icon),
        ParitySurface::Toolbar => ("toolbar", ParityViewMode::Icon),
        ParitySurface::Thumbnail | ParitySurface::Preview => ("gallery", ParityViewMode::Gallery),
        ParitySurface::Sheet => ("sheet", ParityViewMode::Icon),
        ParitySurface::Menu => ("menu", ParityViewMode::Icon),
    }
}

fn matrix_capture_options(
    options: &ParityCaptureMatrixOptions,
    target: ParityCaptureTarget,
    scenario: &str,
    view_mode: ParityViewMode,
    output_png: PathBuf,
    provenance_tsv: PathBuf,
) -> ParityScreenshotCaptureOptions {
    ParityScreenshotCaptureOptions {
        target,
        fixture_root: options.fixture_root.join(scenario),
        output_png,
        provenance_tsv,
        scenario: scenario.to_string(),
        view_mode,
        macos_build: options.macos_build.clone(),
        hardware_profile: options.hardware_profile.clone(),
        display_profile: options.display_profile.clone(),
        app_version: options.app_version.clone(),
        captured_at: options.captured_at.clone(),
        expires_at: options.expires_at.clone(),
        reviewer: options.reviewer.clone(),
        signer: options.signer.clone(),
        approved_mask_set: options.approved_mask_set.clone(),
        appearance: options.appearance,
        scale: options.scale,
        color_profile: options.color_profile,
        focus: options.focus,
        window_origin_x: options.window_origin_x,
        window_origin_y: options.window_origin_y,
        window_size: options.window_size,
        gfm_app: (target == ParityCaptureTarget::Gfm).then(|| options.gfm_app.clone()),
    }
}

fn parity_capture_cli_command(options: &ParityScreenshotCaptureOptions) -> Vec<String> {
    let mut command = vec![
        "gfm".to_string(),
        "parity-capture".to_string(),
        options.target.as_str().to_string(),
        options.fixture_root.display().to_string(),
        options.output_png.display().to_string(),
        options.provenance_tsv.display().to_string(),
        options.scenario.clone(),
        options.view_mode.as_str().to_string(),
        options.macos_build.clone(),
        options.hardware_profile.clone(),
        options.display_profile.clone(),
        options.app_version.clone(),
        options.captured_at.clone(),
        options.reviewer.clone(),
        options.signer.clone(),
        options.approved_mask_set.clone(),
        options.appearance.as_str().to_string(),
        options.scale.as_str().to_string(),
        options.color_profile.as_str().to_string(),
        options.focus.as_str().to_string(),
        options.window_origin_x.to_string(),
        options.window_origin_y.to_string(),
        options.window_size.width.to_string(),
        options.window_size.height.to_string(),
    ];
    if let Some(app) = &options.gfm_app {
        command.push(app.display().to_string());
    }
    push_capture_expiry_args(&mut command, &options.expires_at);
    command
}

struct MatrixManifestCommandInput<'a> {
    options: &'a ParityCaptureMatrixOptions,
    surface: ParitySurface,
    scenario: &'a str,
    view_mode: ParityViewMode,
    manifest_path: &'a Path,
    mask_path: &'a Path,
    finder_output: &'a Path,
    gfm_output: &'a Path,
}

fn parity_capture_manifest_cli_command(input: &MatrixManifestCommandInput<'_>) -> Vec<String> {
    let options = input.options;
    let mut command = vec![
        "gfm".to_string(),
        "parity-capture-manifest".to_string(),
        input.manifest_path.display().to_string(),
        input.surface.as_str().to_string(),
        input.finder_output.display().to_string(),
        input.gfm_output.display().to_string(),
        input.mask_path.display().to_string(),
        options
            .fixture_root
            .join(input.scenario)
            .display()
            .to_string(),
        input.scenario.to_string(),
        input.view_mode.as_str().to_string(),
        options.macos_build.clone(),
        options.hardware_profile.clone(),
        options.display_profile.clone(),
        options.app_version.clone(),
        options.captured_at.clone(),
        options.reviewer.clone(),
        options.signer.clone(),
        options.approved_mask_set.clone(),
        options.appearance.as_str().to_string(),
        options.scale.as_str().to_string(),
        options.color_profile.as_str().to_string(),
        options.focus.as_str().to_string(),
        options.window_size.width.to_string(),
        options.window_size.height.to_string(),
    ];
    push_capture_expiry_args(&mut command, &options.expires_at);
    command
}

fn push_capture_expiry_args(command: &mut Vec<String>, expires_at: &Option<String>) {
    if let Some(expires_at) = expires_at {
        command.push("--expires-at".to_string());
        command.push(expires_at.clone());
    }
}

fn render_capture_matrix_plan(rows: &[ParityCaptureMatrixRow]) -> String {
    let mut text = "surface\tscenario\tview-mode\tfinder-output\tfinder-provenance\tgfm-output\tgfm-provenance\tmask\tmanifest\tfinder-command\tgfm-command\tmanifest-command\n".to_string();
    for row in rows {
        text.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            row.surface.as_str(),
            escape_tsv_field(&row.scenario),
            row.view_mode.as_str(),
            escape_tsv_field(&row.finder_output.to_string_lossy()),
            escape_tsv_field(&row.finder_provenance.to_string_lossy()),
            escape_tsv_field(&row.gfm_output.to_string_lossy()),
            escape_tsv_field(&row.gfm_provenance.to_string_lossy()),
            escape_tsv_field(&row.mask_path.to_string_lossy()),
            escape_tsv_field(&row.manifest_path.to_string_lossy()),
            escape_tsv_field(&row.finder_command.join(" ")),
            escape_tsv_field(&row.gfm_command.join(" ")),
            escape_tsv_field(&row.manifest_command.join(" "))
        ));
    }
    text
}

fn prepare_capture_command(options: &ParityScreenshotCaptureOptions) -> Result<Vec<String>> {
    match options.target {
        ParityCaptureTarget::Finder => Ok(vec![
            "/usr/bin/osascript".to_string(),
            "-e".to_string(),
            finder_prepare_script(options),
        ]),
        ParityCaptureTarget::Gfm => {
            let app = options.gfm_app.as_ref().ok_or_else(|| {
                GfmError::Format("gfm parity capture requires a GFM app path".to_string())
            })?;
            Ok(vec![
                "/usr/bin/osascript".to_string(),
                "-e".to_string(),
                gfm_prepare_script(app, options),
            ])
        }
    }
}

fn finder_prepare_script(options: &ParityScreenshotCaptureOptions) -> String {
    let right = options.window_origin_x + options.window_size.width;
    let bottom = options.window_origin_y + options.window_size.height;
    format!(
        "tell application \"Finder\"\nactivate\nopen POSIX file \"{}\"\nset current view of front Finder window to {}\nset bounds of front Finder window to {{{}, {}, {}, {}}}\ndelay 0.35\nend tell",
        applescript_string(&options.fixture_root),
        finder_view_mode_script(options.view_mode),
        options.window_origin_x,
        options.window_origin_y,
        right,
        bottom
    )
}

fn finder_view_mode_script(view_mode: ParityViewMode) -> &'static str {
    match view_mode {
        ParityViewMode::Icon => "icon view",
        ParityViewMode::List => "list view",
        ParityViewMode::Column => "column view",
        ParityViewMode::Gallery => "flow view",
    }
}

fn gfm_prepare_script(app: &Path, options: &ParityScreenshotCaptureOptions) -> String {
    let app_name = app
        .file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("GFM");
    let fallback_executable_name = app_name.to_ascii_lowercase();
    let executable_path = app_executable_path(app).unwrap_or_else(|| {
        app.join("Contents")
            .join("MacOS")
            .join(&fallback_executable_name)
    });
    let executable_name = executable_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&fallback_executable_name);
    format!(
        "do shell script \"GFM_CAPTURE_WINDOW_X={} GFM_CAPTURE_WINDOW_Y={} GFM_CAPTURE_WINDOW_WIDTH={} GFM_CAPTURE_WINDOW_HEIGHT={} \" & quoted form of \"{}\" & \" \" & quoted form of \"{}\" & \" >/dev/null 2>&1 &\"\ntell application \"System Events\"\n  repeat 30 times\n    if exists process \"{}\" then\n      try\n        set visible of (first process whose name is \"{}\") to true\n        set frontmost of (first process whose name is \"{}\") to true\n      end try\n      if (count of windows of (first process whose name is \"{}\")) > 0 then exit repeat\n    end if\n    delay 0.1\n  end repeat\nend tell\ndelay 0.5",
        options.window_origin_x,
        options.window_origin_y,
        options.window_size.width,
        options.window_size.height,
        applescript_string(&executable_path),
        applescript_string(&options.fixture_root),
        applescript_string_value(executable_name),
        applescript_string_value(executable_name),
        applescript_string_value(executable_name),
        applescript_string_value(executable_name)
    )
}

fn app_executable_path(app: &Path) -> Option<PathBuf> {
    let macos = app.join("Contents").join("MacOS");
    std::fs::read_dir(macos)
        .ok()?
        .filter_map(|entry| entry.ok())
        .find_map(|entry| {
            let path = entry.path();
            path.is_file().then_some(path)
        })
}

fn screencapture_command(region: &CaptureRegion, output_png: &Path) -> Vec<String> {
    vec![
        "/usr/sbin/screencapture".to_string(),
        "-x".to_string(),
        "-R".to_string(),
        region.screencapture_region(),
        output_png.display().to_string(),
    ]
}

fn run_command(command: &[String], label: &str) -> Result<()> {
    let Some((program, args)) = command.split_first() else {
        return Err(GfmError::Format(format!("{label} command cannot be empty")));
    };
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|err| GfmError::Format(command_launch_error(label, program, err)))?;
    if !output.status.success() {
        return Err(GfmError::Format(command_status_error(
            label,
            program,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim(),
        )));
    }
    Ok(())
}

fn command_launch_error(label: &str, program: &str, err: io::Error) -> String {
    let state = match err.kind() {
        io::ErrorKind::NotFound => "missing",
        io::ErrorKind::PermissionDenied => "denied",
        _ => "unavailable",
    };
    format!("{label} {state}: could not launch `{program}`; {err}")
}

fn command_status_error(label: &str, program: &str, status: ExitStatus, stderr: &str) -> String {
    let state = classify_command_failure(stderr);
    format!("{label} {state}: command `{program}` failed with status {status}; stderr={stderr}")
}

fn classify_command_failure(stderr: &str) -> &'static str {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("not authorized")
        || lower.contains("not authorised")
        || lower.contains("not permitted")
        || lower.contains("operation not permitted")
        || lower.contains("screen recording")
        || lower.contains("privacy")
        || lower.contains("tcc")
    {
        return "denied";
    }
    if lower.contains("could not create image from display")
        || lower.contains("no display")
        || lower.contains("display is unavailable")
        || lower.contains("window server")
        || lower.contains("cannot connect to display")
        || lower.contains("can't get application")
        || lower.contains("can’t get application")
    {
        return "unavailable";
    }
    if lower.contains("does not exist")
        || lower.contains("no such file")
        || lower.contains("not found")
        || lower.contains("missing")
    {
        return "missing";
    }
    if lower.contains("offline") {
        return "offline";
    }
    if lower.contains("unsupported") || lower.contains("not supported") {
        return "unsupported";
    }
    "failed"
}

fn write_capture_provenance(
    options: &ParityScreenshotCaptureOptions,
    region: &CaptureRegion,
) -> Result<()> {
    let expires_at = options
        .expires_at
        .as_deref()
        .map(|value| format!("expires-at\t{}\n", escape_tsv_field(value)))
        .unwrap_or_default();
    let content = format!(
        "target\t{}\nfixture-root\t{}\noutput\t{}\nscenario\t{}\nview-mode\t{}\nmacos-build\t{}\nhardware-profile\t{}\ndisplay-profile\t{}\napp-version\t{}\ncaptured-at\t{}\n{}capture-command\t{}\nreviewer\t{}\nsigner\t{}\napproved-mask-set\t{}\nappearance\t{}\nscale\t{}\ncolor-profile\t{}\nfocus\t{}\nwindow-region\t{}\n",
        options.target.as_str(),
        escape_tsv_field(&options.fixture_root.to_string_lossy()),
        escape_tsv_field(&options.output_png.to_string_lossy()),
        escape_tsv_field(&options.scenario),
        options.view_mode.as_str(),
        escape_tsv_field(&options.macos_build),
        escape_tsv_field(&options.hardware_profile),
        escape_tsv_field(&options.display_profile),
        escape_tsv_field(&options.app_version),
        escape_tsv_field(&options.captured_at),
        expires_at,
        escape_tsv_field(&format!(
            "screencapture:-x:-R:{}",
            region.screencapture_region()
        )),
        escape_tsv_field(&options.reviewer),
        escape_tsv_field(&options.signer),
        escape_tsv_field(&options.approved_mask_set),
        options.appearance.as_str(),
        options.scale.as_str(),
        options.color_profile.as_str(),
        options.focus.as_str(),
        region.screencapture_region()
    );
    fs::write(&options.provenance_tsv, content)
        .map_err(|err| GfmError::io(&options.provenance_tsv, err))
}

fn applescript_string(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn applescript_string_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn escape_tsv_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
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
    (1..=days_in_month(year, month)).contains(&day)
}

fn utc_capture_timestamp_epoch_seconds(value: &str) -> i64 {
    let bytes = value.as_bytes();
    let year = i64::from(parse_fixed_u32(bytes, 0, 4));
    let month = i64::from(parse_fixed_u32(bytes, 5, 7));
    let day = i64::from(parse_fixed_u32(bytes, 8, 10));
    let hour = i64::from(parse_fixed_u32(bytes, 11, 13));
    let minute = i64::from(parse_fixed_u32(bytes, 14, 16));
    let second = i64::from(parse_fixed_u32(bytes, 17, 19));
    days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second
}

fn parse_fixed_u32(bytes: &[u8], start: usize, end: usize) -> u32 {
    bytes[start..end]
        .iter()
        .fold(0, |value, byte| value * 10 + u32::from(byte - b'0'))
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_prime = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn plans_real_finder_screenshot_capture_commands() {
        let root = unique_temp_dir("gfm-parity-finder-capture-plan");
        let options = sample_options(
            ParityCaptureTarget::Finder,
            &root,
            root.join("finder.png"),
            root.join("finder.provenance.tsv"),
        );

        let (prepare, capture) = plan_parity_capture_commands(&options).unwrap();

        assert_eq!(prepare[0], "/usr/bin/osascript");
        assert!(prepare[2].contains("tell application \"Finder\""));
        assert!(prepare[2].contains("set current view of front Finder window to list view"));
        assert!(prepare[2].contains("set bounds of front Finder window to {40, 70, 1080, 790}"));
        assert_eq!(
            capture,
            vec![
                "/usr/sbin/screencapture",
                "-x",
                "-R",
                "40,70,1040,720",
                root.join("finder.png").to_str().unwrap()
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn plans_gfm_capture_with_explicit_app_path() {
        let root = unique_temp_dir("gfm-parity-gfm-capture-plan");
        let mut options = sample_options(
            ParityCaptureTarget::Gfm,
            &root,
            root.join("gfm.png"),
            root.join("gfm.provenance.tsv"),
        );
        options.gfm_app = Some(PathBuf::from("/Applications/GFM.app"));

        let (prepare, capture) = plan_parity_capture_commands(&options).unwrap();

        assert_eq!(prepare[0], "/usr/bin/osascript");
        assert!(prepare[2].contains("do shell script \"GFM_CAPTURE_WINDOW_X=40"));
        assert!(prepare[2].contains("/Applications/GFM.app/Contents/MacOS/gfm"));
        assert!(prepare[2].contains(root.to_str().unwrap()));
        assert!(prepare[2].contains("System Events"));
        assert!(prepare[2].contains("repeat 30 times"));
        assert!(prepare[2].contains("count of windows"));
        assert!(prepare[2].contains("GFM_CAPTURE_WINDOW_X=40"));
        assert!(prepare[2].contains("GFM_CAPTURE_WINDOW_Y=70"));
        assert!(prepare[2].contains("GFM_CAPTURE_WINDOW_WIDTH=1040"));
        assert!(prepare[2].contains("GFM_CAPTURE_WINDOW_HEIGHT=720"));
        assert!(prepare[2].contains("frontmost"));
        assert!(prepare[2].contains("delay 0.5"));
        assert_eq!(capture[0], "/usr/sbin/screencapture");
        assert_eq!(capture[3], "40,70,1040,720");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn capture_options_reject_unresolved_system_appearance() {
        let root = unique_temp_dir("gfm-parity-capture-system-appearance");
        let mut options = sample_options(
            ParityCaptureTarget::Finder,
            &root,
            root.join("finder.png"),
            root.join("finder.provenance.tsv"),
        );
        options.appearance = ParityAppearance::System;

        let err = plan_parity_capture_commands(&options).unwrap_err();

        assert!(err
            .to_string()
            .contains("appearance must be resolved light or dark"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn writes_gate_manifest_from_paired_finder_and_gfm_captures() {
        let root = unique_temp_dir("gfm-parity-capture-pair-manifest");
        fs::write(
            root.join("manifest.tsv"),
            "scenario\troot\tview\nlist\t.\tlist\n",
        )
        .unwrap();
        let finder = sample_options(
            ParityCaptureTarget::Finder,
            &root,
            root.join("finder.png"),
            root.join("finder.provenance.tsv"),
        );
        let mut gfm = sample_options(
            ParityCaptureTarget::Gfm,
            &root,
            root.join("gfm.png"),
            root.join("gfm.provenance.tsv"),
        );
        gfm.gfm_app = Some(PathBuf::from("/Applications/GFM.app"));
        let manifest = root.join("gate.tsv");

        write_parity_capture_pair_manifest(&ParityCapturePairManifestOptions {
            manifest_path: manifest.clone(),
            surface: ParitySurface::Text,
            finder,
            gfm,
            mask_path: Some(root.join("mask.tsv")),
        })
        .unwrap();

        let content = fs::read_to_string(manifest).unwrap();
        assert!(content.starts_with("manifest-version\t1\nprofile\tmacos-build=24D70\t"));
        assert!(content.contains("\tfixture-manifest="));
        assert!(content.contains("\texpires-at=2026-09-27T00:00:00Z\t"));
        assert!(content.contains("\tcapture-command=finder:screencapture:-x:-R:40,70,1040,720;gfm:screencapture:-x:-R:40,70,1040,720\t"));
        assert!(content.contains("\nentry\ttext\t"));
        assert!(content.contains(&format!("\t{}\t", root.join("finder.png").display())));
        assert!(content.contains(&format!("\t{}\t", root.join("gfm.png").display())));
        assert!(content.contains(&format!(
            "\t{}\t1040\t720\tactive\tlist\t",
            root.join("mask.tsv").display()
        )));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn capture_pair_manifest_omits_absent_expiry() {
        let root = unique_temp_dir("gfm-parity-capture-pair-no-expiry");
        fs::write(
            root.join("manifest.tsv"),
            "scenario\troot\tview\nlist\t.\tlist\n",
        )
        .unwrap();
        let mut finder = sample_options(
            ParityCaptureTarget::Finder,
            &root,
            root.join("finder.png"),
            root.join("finder.provenance.tsv"),
        );
        let mut gfm = sample_options(
            ParityCaptureTarget::Gfm,
            &root,
            root.join("gfm.png"),
            root.join("gfm.provenance.tsv"),
        );
        finder.expires_at = None;
        gfm.expires_at = None;
        let manifest = root.join("gate.tsv");

        write_parity_capture_pair_manifest(&ParityCapturePairManifestOptions {
            manifest_path: manifest.clone(),
            surface: ParitySurface::Text,
            finder,
            gfm,
            mask_path: None,
        })
        .unwrap();

        let content = fs::read_to_string(manifest).unwrap();
        assert!(!content.contains("expires-at="), "{content}");
        assert!(content.contains("\tcaptured-at=2026-09-09T00:00:00Z\tcapture-command="));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn capture_pair_manifest_rejects_mismatched_metadata() {
        let root = unique_temp_dir("gfm-parity-capture-pair-mismatch");
        let finder = sample_options(
            ParityCaptureTarget::Finder,
            &root,
            root.join("finder.png"),
            root.join("finder.provenance.tsv"),
        );
        let mut gfm = sample_options(
            ParityCaptureTarget::Gfm,
            &root,
            root.join("gfm.png"),
            root.join("gfm.provenance.tsv"),
        );
        gfm.gfm_app = Some(PathBuf::from("/Applications/GFM.app"));
        gfm.focus = ParityFocusState::Inactive;

        let err = write_parity_capture_pair_manifest(&ParityCapturePairManifestOptions {
            manifest_path: root.join("gate.tsv"),
            surface: ParitySurface::Text,
            finder,
            gfm,
            mask_path: None,
        })
        .unwrap_err();

        assert!(err
            .to_string()
            .contains("requires matching Finder and GFM capture metadata"));
        assert!(!root.join("gate.tsv").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn writes_capture_matrix_plan_for_every_surface() {
        let root = unique_temp_dir("gfm-parity-capture-matrix");
        let fixture_root = root.join("fixtures");
        fs::create_dir_all(&fixture_root).unwrap();
        fs::write(
            fixture_root.join("manifest.tsv"),
            "scenario\troot\tfinder-view\tfiles\tdirectories\n",
        )
        .unwrap();
        for surface in ParitySurface::ALL {
            let (scenario, _) = capture_target_for_surface(surface);
            fs::create_dir_all(fixture_root.join(scenario)).unwrap();
        }
        let report = write_parity_capture_matrix_plan(&ParityCaptureMatrixOptions {
            plan_path: root.join("capture-plan.tsv"),
            fixture_root: fixture_root.clone(),
            artifact_root: root.join("artifacts"),
            macos_build: "25A354".to_string(),
            hardware_profile: "macbookpro18,3".to_string(),
            display_profile: "studio-display-p3".to_string(),
            app_version: "0.1.0".to_string(),
            captured_at: "2026-09-09T00:00:00Z".to_string(),
            expires_at: Some("2026-09-27T00:00:00Z".to_string()),
            reviewer: "codex".to_string(),
            signer: "codex".to_string(),
            approved_mask_set: "macos-25A354-default".to_string(),
            appearance: ParityAppearance::Dark,
            scale: DisplayScale::Two,
            color_profile: ColorProfile::DisplayP3,
            focus: ParityFocusState::Active,
            window_origin_x: 40,
            window_origin_y: 70,
            window_size: PixelSize::new(1040, 720),
            gfm_app: PathBuf::from("/Applications/GFM.app"),
        })
        .unwrap();

        assert_eq!(report.rows.len(), ParitySurface::ALL.len());
        let toolbar = report
            .rows
            .iter()
            .find(|row| row.surface == ParitySurface::Toolbar)
            .unwrap();
        assert_eq!(toolbar.scenario, "toolbar");
        assert_eq!(toolbar.view_mode, ParityViewMode::Icon);
        assert!(toolbar
            .finder_command
            .iter()
            .any(|arg| arg == "parity-capture"));
        assert!(toolbar
            .gfm_command
            .iter()
            .any(|arg| arg == "/Applications/GFM.app"));
        assert!(toolbar
            .manifest_command
            .iter()
            .any(|arg| arg == "parity-capture-manifest"));
        let content = fs::read_to_string(report.plan_path).unwrap();
        assert!(content.starts_with("surface\tscenario\tview-mode\tfinder-output\t"));
        assert!(content.contains("\ntoolbar\ttoolbar\ticon\t"));
        assert!(content.contains("display-p3 active 40 70 1040 720"));
        assert!(content.contains("--expires-at 2026-09-27T00:00:00Z"));
        assert!(content.contains("mask-25A354-dark.tsv"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn executes_capture_matrix_into_gate_manifests_and_review_bundles() {
        let root = unique_temp_dir("gfm-parity-capture-matrix-execute");
        let fixture_root = root.join("fixtures");
        write_capture_matrix_fixture_manifest(&fixture_root);
        let options = ParityCaptureMatrixExecutionOptions {
            matrix: ParityCaptureMatrixOptions {
                plan_path: root.join("capture-plan.tsv"),
                fixture_root: fixture_root.clone(),
                artifact_root: root.join("artifacts"),
                macos_build: "25A354".to_string(),
                hardware_profile: "macbookpro18,3".to_string(),
                display_profile: "studio-display-p3".to_string(),
                app_version: "0.1.0".to_string(),
                captured_at: "2026-09-09T00:00:00Z".to_string(),
                expires_at: Some("2026-09-27T00:00:00Z".to_string()),
                reviewer: "codex".to_string(),
                signer: "codex".to_string(),
                approved_mask_set: "macos-25A354-default".to_string(),
                appearance: ParityAppearance::Dark,
                scale: DisplayScale::Two,
                color_profile: ColorProfile::DisplayP3,
                focus: ParityFocusState::Active,
                window_origin_x: 40,
                window_origin_y: 70,
                window_size: PixelSize::new(2, 1),
                gfm_app: PathBuf::from("/Applications/GFM.app"),
            },
            review_root: root.join("review"),
        };

        let report = execute_parity_capture_matrix_with_capture(
            &options,
            &mut || Ok(()),
            |capture, check| {
                check()?;
                write_synthetic_capture_png(&capture.output_png, capture.window_size);
                let region = CaptureRegion {
                    x: capture.window_origin_x,
                    y: capture.window_origin_y,
                    width: capture.window_size.width,
                    height: capture.window_size.height,
                };
                write_capture_provenance(capture, &region)?;
                Ok(ParityScreenshotCaptureReport {
                    target: capture.target,
                    fixture_root: capture.fixture_root.clone(),
                    output_png: capture.output_png.clone(),
                    provenance_tsv: capture.provenance_tsv.clone(),
                    window_region: region,
                    prepare_command: vec!["synthetic-prepare".to_string()],
                    capture_command: vec!["synthetic-capture".to_string()],
                })
            },
        )
        .unwrap();

        assert_eq!(report.rows.len(), ParitySurface::ALL.len());
        assert!(report.rows.iter().all(|row| row.passed));
        assert!(report.rows.iter().all(|row| row.violations == 0));
        let toolbar = report
            .rows
            .iter()
            .find(|row| row.surface == ParitySurface::Toolbar)
            .unwrap();
        assert!(toolbar.manifest_path.exists());
        assert!(toolbar.review_dir.join("review.md").exists());
        assert!(toolbar
            .review_dir
            .join("visual-diffs")
            .join("000-toolbar-diff.png")
            .exists());
        assert!(fs::read_to_string(&toolbar.manifest_path)
            .unwrap()
            .contains("\nentry\ttoolbar\t"));
        assert!(fs::read_to_string(report.plan_path)
            .unwrap()
            .contains("\ntoolbar\ttoolbar\ticon\t"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn actual_gfm_capture_rejects_missing_app_bundle_as_missing() {
        let root = unique_temp_dir("gfm-parity-capture-missing-app");
        let mut options = sample_options(
            ParityCaptureTarget::Gfm,
            &root,
            root.join("gfm.png"),
            root.join("gfm.provenance.tsv"),
        );
        options.gfm_app = Some(root.join("Missing.app"));

        let err = capture_parity_screenshot_checked(&options, || Ok(())).unwrap_err();

        let message = err.to_string();
        assert!(message.contains("GFM app missing"), "{message}");
        assert!(message.contains("Missing.app"), "{message}");
        assert!(!root.join("gfm.png").exists());
        assert!(!root.join("gfm.provenance.tsv").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn capture_command_errors_preserve_host_state() {
        let display = command_status_error(
            "parity screenshot capture",
            "/usr/sbin/screencapture",
            synthetic_failure_status(),
            "could not create image from display with rect (40.0, 70.0, 800.0, 500.0)",
        );
        assert!(
            display.contains("parity screenshot capture unavailable"),
            "{display}"
        );

        let tcc = command_status_error(
            "parity capture prepare",
            "/usr/bin/osascript",
            synthetic_failure_status(),
            "Not authorized to send Apple events to Finder.",
        );
        assert!(tcc.contains("parity capture prepare denied"), "{tcc}");

        let unsupported = command_status_error(
            "parity capture prepare",
            "/usr/bin/osascript",
            synthetic_failure_status(),
            "flow view is not supported on this macOS build",
        );
        assert!(
            unsupported.contains("parity capture prepare unsupported"),
            "{unsupported}"
        );

        let unscriptable = command_status_error(
            "parity capture prepare",
            "/usr/bin/osascript",
            synthetic_failure_status(),
            "execution error: Can’t get application \"GFM\". (-1728)",
        );
        assert!(
            unscriptable.contains("parity capture prepare unavailable"),
            "{unscriptable}"
        );
    }

    #[test]
    fn capture_launch_errors_preserve_host_state() {
        let missing = command_launch_error(
            "parity capture prepare",
            "/missing/osascript",
            io::Error::from(io::ErrorKind::NotFound),
        );
        assert!(
            missing.contains("parity capture prepare missing"),
            "{missing}"
        );

        let denied = command_launch_error(
            "parity screenshot capture",
            "/usr/sbin/screencapture",
            io::Error::from(io::ErrorKind::PermissionDenied),
        );
        assert!(
            denied.contains("parity screenshot capture denied"),
            "{denied}"
        );
    }

    #[cfg(unix)]
    fn synthetic_failure_status() -> ExitStatus {
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(256)
    }

    #[cfg(not(unix))]
    fn synthetic_failure_status() -> ExitStatus {
        std::process::Command::new("false").status().unwrap()
    }

    fn sample_options(
        target: ParityCaptureTarget,
        fixture_root: &Path,
        output_png: PathBuf,
        provenance_tsv: PathBuf,
    ) -> ParityScreenshotCaptureOptions {
        ParityScreenshotCaptureOptions {
            target,
            fixture_root: fixture_root.to_path_buf(),
            output_png,
            provenance_tsv,
            scenario: "list".to_string(),
            view_mode: ParityViewMode::List,
            macos_build: "24D70".to_string(),
            hardware_profile: "macbookpro18,3".to_string(),
            display_profile: "studio-display-p3".to_string(),
            app_version: "0.1.0".to_string(),
            captured_at: "2026-09-09T00:00:00Z".to_string(),
            expires_at: Some("2026-09-27T00:00:00Z".to_string()),
            reviewer: "codex".to_string(),
            signer: "codex".to_string(),
            approved_mask_set: "macos-24D70-default".to_string(),
            appearance: ParityAppearance::Dark,
            scale: DisplayScale::Two,
            color_profile: ColorProfile::DisplayP3,
            focus: ParityFocusState::Active,
            window_origin_x: 40,
            window_origin_y: 70,
            window_size: PixelSize::new(1040, 720),
            gfm_app: None,
        }
    }

    fn write_capture_matrix_fixture_manifest(fixture_root: &Path) {
        fs::create_dir_all(fixture_root).unwrap();
        let mut manifest = "scenario\troot\tfinder-view\tfiles\tdirectories\n".to_string();
        let mut scenarios = std::collections::BTreeSet::new();
        for surface in ParitySurface::ALL {
            let (scenario, view_mode) = capture_target_for_surface(surface);
            if scenarios.insert(scenario) {
                fs::create_dir_all(fixture_root.join(scenario)).unwrap();
                manifest.push_str(&format!(
                    "{scenario}\t{scenario}\t{}\t0\t0\n",
                    view_mode.as_str()
                ));
            }
        }
        fs::write(fixture_root.join("manifest.tsv"), manifest).unwrap();
    }

    fn write_synthetic_capture_png(path: &Path, size: PixelSize) {
        let image = crate::RgbaImage {
            size,
            bytes: (0..size.pixel_count().unwrap())
                .flat_map(|_| [32, 34, 38, 255])
                .collect(),
        };
        let diff = crate::diff_rgba(
            &image.bytes,
            &image.bytes,
            &crate::PixelDiffOptions::strict(size),
        )
        .unwrap();
        crate::write_visual_diff_png(path, &image, &image, &diff).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "{}-{}-{}",
            prefix,
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }
}
