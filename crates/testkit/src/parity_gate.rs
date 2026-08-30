use crate::{
    diff_image_files, evaluate_pixel_threshold, read_governed_mask_file, write_visual_diff_png,
    ColorProfile, DisplayScale, ParityAppearance, ParitySurface, PixelDiffReport,
    PixelDriftThreshold, PixelSize, PixelThresholdEvaluation,
};
use gfm_types::{GfmError, Result};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParityGateInput {
    pub surface: ParitySurface,
    pub expected_path: PathBuf,
    pub actual_path: PathBuf,
    pub size: PixelSize,
    pub mask_path: Option<PathBuf>,
    pub provenance: Option<ParityCaptureProvenance>,
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
            provenance: None,
        }
    }

    pub fn with_mask(mut self, mask_path: impl Into<PathBuf>) -> Self {
        self.mask_path = Some(mask_path.into());
        self
    }

    pub fn with_provenance(mut self, provenance: ParityCaptureProvenance) -> Self {
        self.provenance = Some(provenance);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParityCaptureProvenance {
    pub macos_build: String,
    pub hardware_profile: String,
    pub display_profile: String,
    pub app_version: String,
    pub fixture_manifest: String,
    pub captured_at: String,
    pub capture_command: String,
    pub reviewer: String,
    pub signer: String,
    pub approved_mask_set: String,
    pub appearance: ParityAppearance,
    pub scale: DisplayScale,
    pub color_profile: ColorProfile,
    pub window_size: PixelSize,
    pub focus: ParityFocusState,
    pub view_mode: ParityViewMode,
    pub fixture_root: PathBuf,
}

impl ParityCaptureProvenance {
    pub fn validate(&self) -> Result<()> {
        if self.macos_build.trim().is_empty() {
            return Err(GfmError::Format(
                "parity manifest macOS build cannot be empty".to_string(),
            ));
        }
        if self.hardware_profile.trim().is_empty() {
            return Err(GfmError::Format(
                "parity manifest hardware profile cannot be empty".to_string(),
            ));
        }
        if self.display_profile.trim().is_empty() {
            return Err(GfmError::Format(
                "parity manifest display profile cannot be empty".to_string(),
            ));
        }
        if self.app_version.trim().is_empty() {
            return Err(GfmError::Format(
                "parity manifest app version cannot be empty".to_string(),
            ));
        }
        if self.fixture_manifest.trim().is_empty() {
            return Err(GfmError::Format(
                "parity manifest fixture manifest cannot be empty".to_string(),
            ));
        }
        if self.captured_at.trim().is_empty() {
            return Err(GfmError::Format(
                "parity manifest capture timestamp cannot be empty".to_string(),
            ));
        }
        if self.capture_command.trim().is_empty() {
            return Err(GfmError::Format(
                "parity manifest capture command cannot be empty".to_string(),
            ));
        }
        if self.reviewer.trim().is_empty() {
            return Err(GfmError::Format(
                "parity manifest reviewer cannot be empty".to_string(),
            ));
        }
        if self.signer.trim().is_empty() {
            return Err(GfmError::Format(
                "parity manifest signer cannot be empty".to_string(),
            ));
        }
        if self.approved_mask_set.trim().is_empty() {
            return Err(GfmError::Format(
                "parity manifest approved mask set cannot be empty".to_string(),
            ));
        }
        if self.fixture_root.as_os_str().is_empty() {
            return Err(GfmError::Format(
                "parity manifest fixture root cannot be empty".to_string(),
            ));
        }
        if self.window_size.width == 0 || self.window_size.height == 0 {
            return Err(GfmError::Format(
                "parity manifest window size must be positive".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParityFocusState {
    Active,
    Inactive,
}

impl ParityFocusState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
        }
    }
}

impl FromStr for ParityFocusState {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "active" => Ok(Self::Active),
            "inactive" => Ok(Self::Inactive),
            _ => Err(format!("unknown parity focus state: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParityViewMode {
    Icon,
    List,
    Column,
    Gallery,
}

impl ParityViewMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Icon => "icon",
            Self::List => "list",
            Self::Column => "column",
            Self::Gallery => "gallery",
        }
    }
}

impl FromStr for ParityViewMode {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "icon" => Ok(Self::Icon),
            "list" => Ok(Self::List),
            "column" => Ok(Self::Column),
            "gallery" => Ok(Self::Gallery),
            _ => Err(format!("unknown parity view mode: {value}")),
        }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParityReviewBundle {
    pub output_dir: PathBuf,
    pub review_path: PathBuf,
    pub entries_path: PathBuf,
    pub violations_path: PathBuf,
    pub first_mismatch_path: PathBuf,
    pub region_summary_path: PathBuf,
    pub mask_justification_path: PathBuf,
    pub visual_diff_dir: PathBuf,
    pub source_artifact_dir: PathBuf,
    pub bundle_manifest_path: PathBuf,
    pub report: ParityGateReport,
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
        validate_distinct_capture_artifacts(&input)?;
        let masks = input
            .mask_path
            .as_ref()
            .map(|path| read_governed_mask_file(path, input.size))
            .transpose()?
            .unwrap_or_default();
        let (diff, _, _) = diff_image_files(
            &input.expected_path,
            &input.actual_path,
            Some(input.size),
            masks,
        )?;
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

fn validate_distinct_capture_artifacts(input: &ParityGateInput) -> Result<()> {
    if input.expected_path == input.actual_path {
        return Err(identical_capture_artifact_error(input));
    }
    let expected = fs::metadata(&input.expected_path)
        .map_err(|err| GfmError::io(&input.expected_path, err))?;
    let actual =
        fs::metadata(&input.actual_path).map_err(|err| GfmError::io(&input.actual_path, err))?;
    if same_file_identity(&expected, &actual) {
        return Err(identical_capture_artifact_error(input));
    }
    Ok(())
}

fn identical_capture_artifact_error(input: &ParityGateInput) -> GfmError {
    GfmError::Format(format!(
        "parity gate entry for {} must compare distinct Finder and GFM capture artifacts: {} and {}",
        input.surface.as_str(),
        input.expected_path.display(),
        input.actual_path.display()
    ))
}

#[cfg(unix)]
fn same_file_identity(expected: &fs::Metadata, actual: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    expected.dev() == actual.dev() && expected.ino() == actual.ino()
}

#[cfg(not(unix))]
fn same_file_identity(expected: &fs::Metadata, actual: &fs::Metadata) -> bool {
    expected.len() == actual.len()
        && expected
            .modified()
            .ok()
            .zip(actual.modified().ok())
            .is_some_and(|(expected, actual)| expected == actual)
}

pub fn write_parity_review_bundle_manifest(
    manifest_path: impl AsRef<Path>,
    output_dir: impl AsRef<Path>,
) -> Result<ParityReviewBundle> {
    let report = run_parity_gate_manifest(manifest_path.as_ref())?;
    write_parity_review_bundle(report, output_dir)
}

pub fn write_parity_review_bundle(
    report: ParityGateReport,
    output_dir: impl AsRef<Path>,
) -> Result<ParityReviewBundle> {
    let output_dir = output_dir.as_ref().to_path_buf();
    fs::create_dir_all(&output_dir).map_err(|err| GfmError::io(&output_dir, err))?;

    let review_path = output_dir.join("review.md");
    let entries_path = output_dir.join("entries.tsv");
    let violations_path = output_dir.join("violations.tsv");
    let first_mismatch_path = output_dir.join("first-unmasked.tsv");
    let region_summary_path = output_dir.join("regions.tsv");
    let mask_justification_path = output_dir.join("mask-justifications.tsv");
    let visual_diff_dir = output_dir.join("visual-diffs");
    let source_artifact_dir = output_dir.join("source-artifacts");
    let bundle_manifest_path = output_dir.join("bundle.tsv");

    fs::create_dir_all(&visual_diff_dir).map_err(|err| GfmError::io(&visual_diff_dir, err))?;
    fs::create_dir_all(&source_artifact_dir)
        .map_err(|err| GfmError::io(&source_artifact_dir, err))?;
    let artifact_rows =
        write_review_image_artifacts(&report, &visual_diff_dir, &source_artifact_dir)?;

    write_text(&review_path, &render_review_markdown(&report))?;
    write_text(&entries_path, &render_entries_tsv(&report))?;
    write_text(&violations_path, &render_violations_tsv(&report))?;
    write_text(&first_mismatch_path, &render_first_mismatches_tsv(&report))?;
    write_text(&region_summary_path, &render_regions_tsv(&report))?;
    write_text(
        &mask_justification_path,
        &render_mask_justifications_tsv(&report),
    )?;
    let manifest_context = BundleManifestContext {
        review_path: &review_path,
        entries_path: &entries_path,
        violations_path: &violations_path,
        first_mismatch_path: &first_mismatch_path,
        region_summary_path: &region_summary_path,
        mask_justification_path: &mask_justification_path,
        visual_diff_dir: &visual_diff_dir,
        source_artifact_dir: &source_artifact_dir,
        artifact_rows: &artifact_rows,
    };
    write_text(
        &bundle_manifest_path,
        &render_bundle_manifest(&manifest_context),
    )?;

    Ok(ParityReviewBundle {
        output_dir,
        review_path,
        entries_path,
        violations_path,
        first_mismatch_path,
        region_summary_path,
        mask_justification_path,
        visual_diff_dir,
        source_artifact_dir,
        bundle_manifest_path,
        report,
    })
}

pub fn parse_parity_gate_manifest(content: &str, base: &Path) -> Result<Vec<ParityGateInput>> {
    parse_parity_gate_manifest_with_provenance(content, base)
}

pub fn parse_parity_gate_manifest_with_provenance(
    content: &str,
    base: &Path,
) -> Result<Vec<ParityGateInput>> {
    let mut inputs = Vec::new();
    let mut profile: Option<ManifestProfile> = None;
    for (line_index, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.first() == Some(&"manifest-version") {
            if fields.get(1) != Some(&"1") {
                return Err(GfmError::Format(format!(
                    "parity gate manifest line {} has unsupported manifest version",
                    line_index + 1
                )));
            }
            continue;
        }
        if fields.first() == Some(&"profile") {
            if profile.is_some() {
                return Err(GfmError::Format(format!(
                    "parity gate manifest line {} has duplicate capture profile",
                    line_index + 1
                )));
            }
            profile = Some(parse_manifest_profile(line_index, &fields)?);
            continue;
        }
        let fields = if fields.first() == Some(&"entry") {
            &fields[1..]
        } else {
            &fields[..]
        };
        if fields.len() != 5 && fields.len() != 6 {
            if fields.len() == 11 {
                let profile = profile.as_ref().ok_or_else(|| {
                    GfmError::Format(format!(
                        "parity gate manifest line {} has versioned entry without profile",
                        line_index + 1
                    ))
                })?;
                let input = parse_versioned_entry(line_index, fields, base, profile)?;
                inputs.push(input);
                continue;
            }
            return Err(GfmError::Format(format!(
                "parity gate manifest line {} must contain surface, expected, actual, width, height, and optional mask",
                line_index + 1
            )));
        }
        return Err(GfmError::Format(format!(
            "parity gate manifest line {} is missing capture provenance; use manifest-version 1, a profile row, and versioned entry rows",
            line_index + 1
        )));
    }
    if inputs.is_empty() {
        return Err(GfmError::Format(
            "parity gate manifest does not contain any entries".to_string(),
        ));
    }
    Ok(inputs)
}

fn parse_versioned_entry(
    line_index: usize,
    fields: &[&str],
    base: &Path,
    profile: &ManifestProfile,
) -> Result<ParityGateInput> {
    let surface = ParitySurface::from_str(fields[0]).map_err(GfmError::Format)?;
    let expected_path = resolve_manifest_path(base, fields[1]);
    let actual_path = resolve_manifest_path(base, fields[2]);
    let width = parse_manifest_u32(line_index, "width", fields[3])?;
    let height = parse_manifest_u32(line_index, "height", fields[4])?;
    let window_width = parse_manifest_u32(line_index, "window-width", fields[6])?;
    let window_height = parse_manifest_u32(line_index, "window-height", fields[7])?;
    let focus = fields[8]
        .parse::<ParityFocusState>()
        .map_err(GfmError::Format)?;
    let view_mode = fields[9]
        .parse::<ParityViewMode>()
        .map_err(GfmError::Format)?;
    let fixture_root = resolve_manifest_path(base, fields[10]);
    let provenance = ParityCaptureProvenance {
        macos_build: profile.macos_build.clone(),
        hardware_profile: profile.hardware_profile.clone(),
        display_profile: profile.display_profile.clone(),
        app_version: profile.app_version.clone(),
        fixture_manifest: profile.fixture_manifest.clone(),
        captured_at: profile.captured_at.clone(),
        capture_command: profile.capture_command.clone(),
        reviewer: profile.reviewer.clone(),
        signer: profile.signer.clone(),
        approved_mask_set: profile.approved_mask_set.clone(),
        appearance: profile.appearance,
        scale: profile.scale,
        color_profile: profile.color_profile,
        window_size: PixelSize::new(window_width, window_height),
        focus,
        view_mode,
        fixture_root,
    };
    provenance.validate()?;
    let mut input = ParityGateInput::new(
        surface,
        expected_path,
        actual_path,
        PixelSize::new(width, height),
    )
    .with_provenance(provenance);
    if !fields[5].is_empty() {
        input = input.with_mask(resolve_manifest_path(base, fields[5]));
    }
    Ok(input)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManifestProfile {
    macos_build: String,
    hardware_profile: String,
    display_profile: String,
    app_version: String,
    fixture_manifest: String,
    captured_at: String,
    capture_command: String,
    reviewer: String,
    signer: String,
    approved_mask_set: String,
    appearance: ParityAppearance,
    scale: DisplayScale,
    color_profile: ColorProfile,
}

fn parse_manifest_profile(line_index: usize, fields: &[&str]) -> Result<ManifestProfile> {
    let mut macos_build = None;
    let mut hardware_profile = None;
    let mut display_profile = None;
    let mut app_version = None;
    let mut fixture_manifest = None;
    let mut captured_at = None;
    let mut capture_command = None;
    let mut reviewer = None;
    let mut signer = None;
    let mut approved_mask_set = None;
    let mut appearance = None;
    let mut scale = None;
    let mut color_profile = None;
    let mut seen_keys = BTreeSet::new();
    for field in fields.iter().skip(1) {
        let Some((key, value)) = field.split_once('=') else {
            return Err(GfmError::Format(format!(
                "parity gate manifest line {} has invalid profile field `{field}`",
                line_index + 1
            )));
        };
        if !seen_keys.insert(key) {
            return Err(GfmError::Format(format!(
                "parity gate manifest line {} has duplicate profile key `{key}`",
                line_index + 1
            )));
        }
        match key {
            "macos-build" => macos_build = Some(value.to_string()),
            "hardware-profile" => hardware_profile = Some(value.to_string()),
            "display-profile" => display_profile = Some(value.to_string()),
            "app-version" => app_version = Some(value.to_string()),
            "fixture-manifest" => fixture_manifest = Some(value.to_string()),
            "captured-at" => captured_at = Some(value.to_string()),
            "capture-command" => capture_command = Some(value.to_string()),
            "reviewer" => reviewer = Some(value.to_string()),
            "signer" => signer = Some(value.to_string()),
            "approved-mask-set" => approved_mask_set = Some(value.to_string()),
            "appearance" => {
                appearance = Some(
                    value
                        .parse::<ParityAppearance>()
                        .map_err(GfmError::Format)?,
                )
            }
            "scale" => scale = Some(value.parse::<DisplayScale>().map_err(GfmError::Format)?),
            "color-profile" => {
                color_profile = Some(value.parse::<ColorProfile>().map_err(GfmError::Format)?)
            }
            _ => {
                return Err(GfmError::Format(format!(
                    "parity gate manifest line {} has unknown profile key `{key}`",
                    line_index + 1
                )))
            }
        }
    }
    let profile = ManifestProfile {
        macos_build: macos_build.ok_or_else(|| {
            GfmError::Format(format!(
                "parity gate manifest line {} missing macos-build",
                line_index + 1
            ))
        })?,
        hardware_profile: hardware_profile.ok_or_else(|| {
            GfmError::Format(format!(
                "parity gate manifest line {} missing hardware-profile",
                line_index + 1
            ))
        })?,
        display_profile: display_profile.ok_or_else(|| {
            GfmError::Format(format!(
                "parity gate manifest line {} missing display-profile",
                line_index + 1
            ))
        })?,
        app_version: app_version.ok_or_else(|| {
            GfmError::Format(format!(
                "parity gate manifest line {} missing app-version",
                line_index + 1
            ))
        })?,
        fixture_manifest: fixture_manifest.ok_or_else(|| {
            GfmError::Format(format!(
                "parity gate manifest line {} missing fixture-manifest",
                line_index + 1
            ))
        })?,
        captured_at: captured_at.ok_or_else(|| {
            GfmError::Format(format!(
                "parity gate manifest line {} missing captured-at",
                line_index + 1
            ))
        })?,
        capture_command: capture_command.ok_or_else(|| {
            GfmError::Format(format!(
                "parity gate manifest line {} missing capture-command",
                line_index + 1
            ))
        })?,
        reviewer: reviewer.ok_or_else(|| {
            GfmError::Format(format!(
                "parity gate manifest line {} missing reviewer",
                line_index + 1
            ))
        })?,
        signer: signer.ok_or_else(|| {
            GfmError::Format(format!(
                "parity gate manifest line {} missing signer",
                line_index + 1
            ))
        })?,
        approved_mask_set: approved_mask_set.ok_or_else(|| {
            GfmError::Format(format!(
                "parity gate manifest line {} missing approved-mask-set",
                line_index + 1
            ))
        })?,
        appearance: appearance.ok_or_else(|| {
            GfmError::Format(format!(
                "parity gate manifest line {} missing appearance",
                line_index + 1
            ))
        })?,
        scale: scale.ok_or_else(|| {
            GfmError::Format(format!(
                "parity gate manifest line {} missing scale",
                line_index + 1
            ))
        })?,
        color_profile: color_profile.ok_or_else(|| {
            GfmError::Format(format!(
                "parity gate manifest line {} missing color-profile",
                line_index + 1
            ))
        })?,
    };
    if profile.macos_build.trim().is_empty() {
        return Err(GfmError::Format(format!(
            "parity gate manifest line {} has empty macos-build",
            line_index + 1
        )));
    }
    if profile.hardware_profile.trim().is_empty() {
        return Err(GfmError::Format(format!(
            "parity gate manifest line {} has empty hardware-profile",
            line_index + 1
        )));
    }
    if profile.display_profile.trim().is_empty() {
        return Err(GfmError::Format(format!(
            "parity gate manifest line {} has empty display-profile",
            line_index + 1
        )));
    }
    if profile.app_version.trim().is_empty() {
        return Err(GfmError::Format(format!(
            "parity gate manifest line {} has empty app-version",
            line_index + 1
        )));
    }
    if profile.fixture_manifest.trim().is_empty() {
        return Err(GfmError::Format(format!(
            "parity gate manifest line {} has empty fixture-manifest",
            line_index + 1
        )));
    }
    if profile.captured_at.trim().is_empty() {
        return Err(GfmError::Format(format!(
            "parity gate manifest line {} has empty captured-at",
            line_index + 1
        )));
    }
    if profile.capture_command.trim().is_empty() {
        return Err(GfmError::Format(format!(
            "parity gate manifest line {} has empty capture-command",
            line_index + 1
        )));
    }
    if profile.reviewer.trim().is_empty() {
        return Err(GfmError::Format(format!(
            "parity gate manifest line {} has empty reviewer",
            line_index + 1
        )));
    }
    if profile.signer.trim().is_empty() {
        return Err(GfmError::Format(format!(
            "parity gate manifest line {} has empty signer",
            line_index + 1
        )));
    }
    if profile.approved_mask_set.trim().is_empty() {
        return Err(GfmError::Format(format!(
            "parity gate manifest line {} has empty approved-mask-set",
            line_index + 1
        )));
    }
    Ok(profile)
}

fn render_review_markdown(report: &ParityGateReport) -> String {
    let manifest = report
        .manifest_path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "<in-memory>".to_string());
    let mut text = String::new();
    text.push_str("# GFM Finder Parity Review\n\n");
    text.push_str(&format!("Manifest: `{manifest}`\n\n"));
    text.push_str(&format!("Entries: {}\n\n", report.entries.len()));
    text.push_str(&format!("Violations: {}\n\n", report.violations()));
    text.push_str(&format!("Passed: {}\n\n", report.passed()));
    if report
        .entries
        .iter()
        .any(|entry| entry.input.provenance.is_some())
    {
        text.push_str("## Capture Provenance\n\n");
        text.push_str("| Surface | macOS Build | Appearance | Scale | Color Profile | Window | Focus | View Mode | Fixture Root | Reviewer | Signer | Approved Masks |\n");
        text.push_str(
            "| --- | --- | --- | --- | --- | ---: | --- | --- | --- | --- | --- | --- |\n",
        );
        for entry in &report.entries {
            if let Some(provenance) = &entry.input.provenance {
                text.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {}x{} | {} | {} | {} | {} | {} | {} |\n",
                    entry.input.surface.as_str(),
                    provenance.macos_build,
                    provenance.appearance.as_str(),
                    provenance.scale.as_str(),
                    provenance.color_profile.as_str(),
                    provenance.window_size.width,
                    provenance.window_size.height,
                    provenance.focus.as_str(),
                    provenance.view_mode.as_str(),
                    provenance.fixture_root.display(),
                    provenance.reviewer,
                    provenance.signer,
                    provenance.approved_mask_set
                ));
            }
        }
        text.push('\n');
    }
    text.push_str("## Surface Summary\n\n");
    text.push_str("| Surface | Size | Mismatched | Unmasked | Masked | Max Delta | Passed |\n");
    text.push_str("| --- | ---: | ---: | ---: | ---: | ---: | --- |\n");
    for entry in &report.entries {
        text.push_str(&format!(
            "| {} | {}x{} | {} | {} | {} | {} | {} |\n",
            entry.input.surface.as_str(),
            entry.diff.size.width,
            entry.diff.size.height,
            entry.diff.mismatched_pixels,
            entry.diff.unmasked_mismatches,
            entry.diff.masked_mismatches,
            entry.diff.max_channel_delta,
            entry.passed()
        ));
    }
    if report.violations() > 0 {
        text.push_str("\n## Required Review\n\n");
        text.push_str(
            "Every unmasked drift must be rejected or backed by a new captured Finder baseline before merge.\n",
        );
    }
    text
}

fn render_entries_tsv(report: &ParityGateReport) -> String {
    let mut text =
        "surface\twidth\theight\texpected\tactual\tmask\tmacos-build\thardware-profile\tdisplay-profile\tapp-version\tfixture-manifest\tcaptured-at\tcapture-command\treviewer\tsigner\tapproved-mask-set\tappearance\tscale\tcolor-profile\twindow-width\twindow-height\tfocus\tview-mode\tfixture-root\tmismatched\tunmasked\tmasked\tmax-channel-delta\tpassed\n"
            .to_string();
    for entry in &report.entries {
        let provenance = entry.input.provenance.as_ref();
        text.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            entry.input.surface.as_str(),
            entry.diff.size.width,
            entry.diff.size.height,
            entry.input.expected_path.display(),
            entry.input.actual_path.display(),
            entry
                .input
                .mask_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            provenance
                .map(|value| value.macos_build.as_str())
                .unwrap_or_default(),
            provenance
                .map(|value| value.hardware_profile.as_str())
                .unwrap_or_default(),
            provenance
                .map(|value| value.display_profile.as_str())
                .unwrap_or_default(),
            provenance
                .map(|value| value.app_version.as_str())
                .unwrap_or_default(),
            provenance
                .map(|value| value.fixture_manifest.as_str())
                .unwrap_or_default(),
            provenance
                .map(|value| value.captured_at.as_str())
                .unwrap_or_default(),
            provenance
                .map(|value| value.capture_command.as_str())
                .unwrap_or_default(),
            provenance
                .map(|value| value.reviewer.as_str())
                .unwrap_or_default(),
            provenance
                .map(|value| value.signer.as_str())
                .unwrap_or_default(),
            provenance
                .map(|value| value.approved_mask_set.as_str())
                .unwrap_or_default(),
            provenance
                .map(|value| value.appearance.as_str())
                .unwrap_or_default(),
            provenance
                .map(|value| value.scale.as_str())
                .unwrap_or_default(),
            provenance
                .map(|value| value.color_profile.as_str())
                .unwrap_or_default(),
            provenance
                .map(|value| value.window_size.width.to_string())
                .unwrap_or_default(),
            provenance
                .map(|value| value.window_size.height.to_string())
                .unwrap_or_default(),
            provenance
                .map(|value| value.focus.as_str())
                .unwrap_or_default(),
            provenance
                .map(|value| value.view_mode.as_str())
                .unwrap_or_default(),
            provenance
                .map(|value| value.fixture_root.display().to_string())
                .unwrap_or_default(),
            entry.diff.mismatched_pixels,
            entry.diff.unmasked_mismatches,
            entry.diff.masked_mismatches,
            entry.diff.max_channel_delta,
            entry.passed()
        ));
    }
    text
}

fn render_violations_tsv(report: &ParityGateReport) -> String {
    let mut text = "surface\tviolation\n".to_string();
    for entry in &report.entries {
        for violation in &entry.evaluation.violations {
            text.push_str(&format!(
                "{}\t{}\n",
                entry.input.surface.as_str(),
                violation.as_tsv()
            ));
        }
    }
    text
}

fn render_first_mismatches_tsv(report: &ParityGateReport) -> String {
    let mut text = "surface\tx\ty\texpected_rgba\tactual_rgba\n".to_string();
    for entry in &report.entries {
        if let Some(mismatch) = entry.diff.first_unmasked_mismatch {
            text.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\n",
                entry.input.surface.as_str(),
                mismatch.x,
                mismatch.y,
                pixel_hex(mismatch.expected),
                pixel_hex(mismatch.actual)
            ));
        }
    }
    text
}

fn render_regions_tsv(report: &ParityGateReport) -> String {
    let mut text =
        "surface\tname\tx\ty\twidth\theight\tmismatched\tmax-channel-delta\n".to_string();
    for entry in &report.entries {
        for region in &entry.diff.regions {
            text.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                entry.input.surface.as_str(),
                escape_tsv_field(&region.name),
                region.rect.x,
                region.rect.y,
                region.rect.width,
                region.rect.height,
                region.mismatched_pixels,
                region.max_channel_delta
            ));
        }
    }
    text
}

fn render_mask_justifications_tsv(report: &ParityGateReport) -> String {
    let mut text = "surface\tx\ty\twidth\theight\treason\n".to_string();
    for entry in &report.entries {
        for mask in &entry.diff.masks {
            text.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\n",
                entry.input.surface.as_str(),
                mask.rect.x,
                mask.rect.y,
                mask.rect.width,
                mask.rect.height,
                escape_tsv_field(&mask.reason)
            ));
        }
    }
    text
}

struct BundleManifestContext<'a> {
    review_path: &'a Path,
    entries_path: &'a Path,
    violations_path: &'a Path,
    first_mismatch_path: &'a Path,
    region_summary_path: &'a Path,
    mask_justification_path: &'a Path,
    visual_diff_dir: &'a Path,
    source_artifact_dir: &'a Path,
    artifact_rows: &'a [String],
}

fn render_bundle_manifest(context: &BundleManifestContext<'_>) -> String {
    let mut text = format!(
        "kind\tpath\nreview\t{}\nentries\t{}\nviolations\t{}\nfirst-unmasked\t{}\nregions\t{}\nmask-justifications\t{}\nvisual-diffs\t{}\nsource-artifacts\t{}\n",
        context.review_path.display(),
        context.entries_path.display(),
        context.violations_path.display(),
        context.first_mismatch_path.display(),
        context.region_summary_path.display(),
        context.mask_justification_path.display(),
        context.visual_diff_dir.display(),
        context.source_artifact_dir.display()
    );
    for row in context.artifact_rows {
        text.push_str(row);
        text.push('\n');
    }
    text
}

fn pixel_hex(pixel: [u8; 4]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}",
        pixel[0], pixel[1], pixel[2], pixel[3]
    )
}

fn write_text(path: &Path, content: &str) -> Result<()> {
    fs::write(path, content).map_err(|err| GfmError::io(path, err))
}

fn write_review_image_artifacts(
    report: &ParityGateReport,
    visual_diff_dir: &Path,
    source_artifact_dir: &Path,
) -> Result<Vec<String>> {
    let mut rows = Vec::new();
    for (index, entry) in report.entries.iter().enumerate() {
        let expected =
            crate::read_rgba_image_file(&entry.input.expected_path, Some(entry.input.size))?;
        let actual = crate::read_rgba_image_file(&entry.input.actual_path, Some(entry.input.size))?;
        let stem = format!("{index:03}-{}", entry.input.surface.as_str());
        let diff_path = visual_diff_dir.join(format!("{stem}-diff.png"));
        write_visual_diff_png(&diff_path, &expected, &actual, &entry.diff)?;
        rows.push(format!("visual-diff\t{}", diff_path.display()));

        let expected_copy = source_artifact_dir.join(format!(
            "{stem}-finder{}",
            artifact_extension(&entry.input.expected_path)
        ));
        let actual_copy = source_artifact_dir.join(format!(
            "{stem}-gfm{}",
            artifact_extension(&entry.input.actual_path)
        ));
        copy_artifact(&entry.input.expected_path, &expected_copy)?;
        copy_artifact(&entry.input.actual_path, &actual_copy)?;
        rows.push(format!("finder-source\t{}", expected_copy.display()));
        rows.push(format!("gfm-source\t{}", actual_copy.display()));
    }
    Ok(rows)
}

fn artifact_extension(path: &Path) -> String {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!(".{extension}"))
        .unwrap_or_else(|| ".rgba".to_string())
}

fn copy_artifact(source: &Path, destination: &Path) -> Result<()> {
    fs::copy(source, destination)
        .map(|_| ())
        .map_err(|err| GfmError::io(destination, err))
}

fn escape_tsv_field(value: &str) -> String {
    value.replace(['\t', '\n', '\r'], " ")
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
    use crate::RgbaImage;

    #[test]
    fn parity_gate_passes_only_explicitly_masked_drift() {
        let root = unique_temp_dir("gfm-parity-gate");
        let expected = root.join("expected.rgba");
        let actual = root.join("actual.rgba");
        let mask = root.join("mask.tsv");
        fs::write(&expected, [0, 0, 0, 255, 10, 10, 10, 255]).unwrap();
        fs::write(&actual, [0, 0, 0, 255, 9, 10, 10, 255]).unwrap();
        fs::write(&mask, "1\t0\t1\t1\tclock owned by system menu extras\n").unwrap();

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
        assert_eq!(report.entries[0].diff.regions[0].mismatched_pixels, 1);

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
    fn parity_gate_rejects_identical_finder_and_gfm_capture_artifacts() {
        let root = unique_temp_dir("gfm-parity-gate-identical-artifacts");
        let capture = root.join("capture.rgba");
        fs::write(&capture, [1, 2, 3, 255]).unwrap();

        let err = run_parity_gate(vec![ParityGateInput::new(
            ParitySurface::Toolbar,
            &capture,
            &capture,
            PixelSize::new(1, 1),
        )])
        .unwrap_err();

        assert!(err
            .to_string()
            .contains("must compare distinct Finder and GFM capture artifacts"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parity_gate_rejects_same_capture_artifact_through_path_alias() {
        let root = unique_temp_dir("gfm-parity-gate-aliased-artifact");
        let capture = root.join("capture.rgba");
        fs::write(&capture, [1, 2, 3, 255]).unwrap();
        let aliased_capture = root.join(".").join("capture.rgba");

        let err = run_parity_gate(vec![ParityGateInput::new(
            ParitySurface::Toolbar,
            &capture,
            &aliased_capture,
            PixelSize::new(1, 1),
        )])
        .unwrap_err();

        assert!(err
            .to_string()
            .contains("must compare distinct Finder and GFM capture artifacts"));

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn parity_gate_rejects_same_capture_artifact_through_hard_link() {
        let root = unique_temp_dir("gfm-parity-gate-hardlink-artifact");
        let finder_capture = root.join("finder.rgba");
        let gfm_capture = root.join("gfm.rgba");
        fs::write(&finder_capture, [1, 2, 3, 255]).unwrap();
        fs::hard_link(&finder_capture, &gfm_capture).unwrap();

        let err = run_parity_gate(vec![ParityGateInput::new(
            ParitySurface::Toolbar,
            &finder_capture,
            &gfm_capture,
            PixelSize::new(1, 1),
        )])
        .unwrap_err();

        assert!(err
            .to_string()
            .contains("must compare distinct Finder and GFM capture artifacts"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parity_gate_manifest_resolves_relative_artifacts() {
        let root = unique_temp_dir("gfm-parity-gate-manifest");
        fs::write(root.join("expected.rgba"), [1, 2, 3, 255]).unwrap();
        fs::write(root.join("actual.rgba"), [1, 2, 3, 255]).unwrap();
        fs::write(
            root.join("gate.tsv"),
            "manifest-version\t1\nprofile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=light\tscale=2x\tcolor-profile=srgb\nentry\ticon\texpected.rgba\tactual.rgba\t1\t1\t\t1040\t720\tactive\ticon\tfixtures/icon\n",
        )
        .unwrap();

        let report = run_parity_gate_manifest(root.join("gate.tsv")).unwrap();

        assert!(report.passed());
        assert_eq!(report.entries.len(), 1);
        assert!(report.entries[0]
            .input
            .expected_path
            .ends_with("expected.rgba"));
        assert!(report.entries[0].input.provenance.is_some());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parity_gate_manifest_rejects_missing_capture_provenance() {
        let root = unique_temp_dir("gfm-parity-gate-missing-provenance");
        let err = parse_parity_gate_manifest("icon\texpected.rgba\tactual.rgba\t1\t1\n", &root)
            .unwrap_err();

        assert!(err.to_string().contains("missing capture provenance"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn versioned_parity_manifest_validates_capture_provenance() {
        let root = unique_temp_dir("gfm-parity-gate-versioned");
        fs::write(root.join("finder.png"), [1, 2, 3, 255]).unwrap();
        fs::write(root.join("gfm.png"), [1, 2, 3, 255]).unwrap();
        fs::write(
            root.join("gate.tsv"),
            "manifest-version\t1\nprofile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=dark\tscale=2x\tcolor-profile=display-p3\nentry\ttoolbar\tfinder.png\tgfm.png\t1\t1\t\t1440\t900\tactive\ticon\tfixtures/icon\n",
        )
        .unwrap();

        let inputs =
            parse_parity_gate_manifest(&fs::read_to_string(root.join("gate.tsv")).unwrap(), &root)
                .unwrap();
        let provenance = inputs[0].provenance.as_ref().unwrap();

        assert_eq!(provenance.macos_build, "25A354");
        assert_eq!(provenance.hardware_profile, "macbookpro18,3");
        assert_eq!(provenance.display_profile, "studio-display-p3");
        assert_eq!(provenance.app_version, "0.1.0");
        assert_eq!(provenance.fixture_manifest, "fixtures/manifest.tsv");
        assert_eq!(provenance.captured_at, "2026-08-27T00:00:00Z");
        assert_eq!(provenance.capture_command, "screencapture:-x");
        assert_eq!(provenance.reviewer, "codex");
        assert_eq!(provenance.signer, "codex");
        assert_eq!(provenance.approved_mask_set, "macos-25A354-default");
        assert_eq!(provenance.appearance, ParityAppearance::Dark);
        assert_eq!(provenance.scale, DisplayScale::Two);
        assert_eq!(provenance.color_profile, ColorProfile::DisplayP3);
        assert_eq!(provenance.window_size, PixelSize::new(1440, 900));
        assert_eq!(provenance.focus, ParityFocusState::Active);
        assert_eq!(provenance.view_mode, ParityViewMode::Icon);
        assert!(provenance.fixture_root.ends_with("fixtures/icon"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn versioned_parity_manifest_rejects_incomplete_capture_profile() {
        let root = unique_temp_dir("gfm-parity-gate-incomplete-profile");
        let err = parse_parity_gate_manifest(
            "manifest-version\t1\nprofile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=dark\tscale=2x\tcolor-profile=display-p3\nentry\ttoolbar\tfinder.png\tgfm.png\t1\t1\t\t1440\t900\tactive\ticon\tfixtures/icon\n",
            &root,
        )
        .unwrap_err();

        assert!(err.to_string().contains("missing fixture-manifest"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn versioned_parity_manifest_rejects_duplicate_capture_profile_keys() {
        let root = unique_temp_dir("gfm-parity-gate-duplicate-profile-key");
        let err = parse_parity_gate_manifest(
            "manifest-version\t1\nprofile\tmacos-build=25A354\tmacos-build=25A999\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=dark\tscale=2x\tcolor-profile=display-p3\nentry\ttoolbar\tfinder.png\tgfm.png\t1\t1\t\t1440\t900\tactive\ticon\tfixtures/icon\n",
            &root,
        )
        .unwrap_err();

        assert!(err.to_string().contains("line 2"));
        assert!(err
            .to_string()
            .contains("duplicate profile key `macos-build`"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn versioned_parity_manifest_rejects_duplicate_capture_profile_rows() {
        let root = unique_temp_dir("gfm-parity-gate-duplicate-profile-row");
        let profile =
            "profile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=dark\tscale=2x\tcolor-profile=display-p3";
        let err = parse_parity_gate_manifest(
            &format!(
                "manifest-version\t1\n{profile}\nentry\ttoolbar\tfinder.png\tgfm.png\t1\t1\t\t1440\t900\tactive\ticon\tfixtures/icon\n{profile}\nentry\ttext\tfinder.png\tgfm.png\t1\t1\t\t1440\t900\tactive\tlist\tfixtures/text\n"
            ),
            &root,
        )
        .unwrap_err();

        assert!(err.to_string().contains("line 4"));
        assert!(err.to_string().contains("duplicate capture profile"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn versioned_parity_manifest_requires_profile_before_entries() {
        let root = unique_temp_dir("gfm-parity-gate-versioned-missing-profile");
        let err = parse_parity_gate_manifest(
            "manifest-version\t1\nentry\ttoolbar\tfinder.png\tgfm.png\t1\t1\t\t1440\t900\tactive\ticon\tfixtures/icon\n",
            &root,
        )
        .unwrap_err();

        assert!(err.to_string().contains("versioned entry without profile"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parity_gate_reports_dimension_mismatches_for_png_inputs() {
        let root = unique_temp_dir("gfm-parity-gate-dimensions");
        let one = RgbaImage {
            size: PixelSize::new(1, 1),
            bytes: vec![0, 0, 0, 255],
        };
        let two = RgbaImage {
            size: PixelSize::new(2, 1),
            bytes: vec![0, 0, 0, 255, 0, 0, 0, 255],
        };
        let expected = root.join("expected.png");
        let actual = root.join("actual.png");
        write_visual_diff_png(&expected, &one, &one, &empty_report(one.size)).unwrap();
        write_visual_diff_png(&actual, &two, &two, &empty_report(two.size)).unwrap();

        let err = run_parity_gate(vec![ParityGateInput::new(
            ParitySurface::Icon,
            &expected,
            &actual,
            PixelSize::new(1, 1),
        )])
        .unwrap_err();

        assert!(err.to_string().contains("do not match declared 1x1"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn review_bundle_writes_human_artifacts_for_failed_drift() {
        let root = unique_temp_dir("gfm-parity-review");
        let expected = root.join("expected.rgba");
        let actual = root.join("actual.rgba");
        let output = root.join("review");
        fs::write(&expected, [0, 0, 0, 255, 10, 10, 10, 255]).unwrap();
        fs::write(&actual, [0, 0, 0, 255, 9, 10, 10, 255]).unwrap();
        fs::write(
            root.join("gate.tsv"),
            "manifest-version\t1\nprofile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=dark\tscale=2x\tcolor-profile=display-p3\nentry\ttext\texpected.rgba\tactual.rgba\t2\t1\t\t1040\t720\tactive\tlist\tfixtures/text\n",
        )
        .unwrap();

        let bundle = write_parity_review_bundle_manifest(root.join("gate.tsv"), &output).unwrap();

        assert!(!bundle.report.passed());
        assert!(bundle.review_path.exists());
        assert!(bundle.entries_path.exists());
        assert!(bundle.violations_path.exists());
        assert!(bundle.first_mismatch_path.exists());
        assert!(bundle.region_summary_path.exists());
        assert!(bundle.mask_justification_path.exists());
        assert!(bundle.visual_diff_dir.join("000-text-diff.png").exists());
        assert!(bundle
            .source_artifact_dir
            .join("000-text-finder.rgba")
            .exists());
        assert!(fs::read_to_string(&bundle.review_path)
            .unwrap()
            .contains("Passed: false"));
        let review_markdown = fs::read_to_string(&bundle.review_path).unwrap();
        assert!(review_markdown.contains("## Capture Provenance"));
        assert!(review_markdown.contains("| text | 25A354 | dark | 2x | display-p3 |"));
        assert!(review_markdown.contains("| codex | codex | macos-25A354-default |"));
        assert!(fs::read_to_string(&bundle.violations_path)
            .unwrap()
            .contains("unmasked-mismatch-budget"));
        assert!(fs::read_to_string(&bundle.first_mismatch_path)
            .unwrap()
            .contains("090a0aff"));

        fs::remove_dir_all(root).unwrap();
    }

    fn empty_report(size: PixelSize) -> PixelDiffReport {
        PixelDiffReport {
            size,
            total_pixels: size.pixel_count().unwrap(),
            mismatched_pixels: 0,
            unmasked_mismatches: 0,
            masked_mismatches: 0,
            max_channel_delta: 0,
            masks: Vec::new(),
            regions: Vec::new(),
            first_unmasked_mismatch: None,
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
