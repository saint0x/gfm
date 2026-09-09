use crate::{
    diff_image_files, evaluate_pixel_threshold, read_governed_mask_file, write_visual_diff_png,
    ColorProfile, DisplayScale, ParityAppearance, ParitySurface, PixelDiffReport,
    PixelDriftThreshold, PixelSize, PixelThresholdEvaluation,
};
use gfm_types::{GfmError, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

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
    pub expires_at: Option<String>,
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
        if !is_valid_utc_capture_timestamp(&self.captured_at) {
            return Err(GfmError::Format(format!(
                "parity manifest capture timestamp must use UTC second precision: {}",
                self.captured_at
            )));
        }
        if let Some(expires_at) = &self.expires_at {
            if !is_valid_utc_capture_timestamp(expires_at) {
                return Err(GfmError::Format(format!(
                    "parity manifest expiry timestamp must use UTC second precision: {expires_at}"
                )));
            }
            if utc_capture_timestamp_epoch_seconds(&self.captured_at)?
                > utc_capture_timestamp_epoch_seconds(expires_at)?
            {
                return Err(GfmError::Format(format!(
                    "parity manifest captured-at `{}` cannot be after expires-at `{}`",
                    self.captured_at, expires_at
                )));
            }
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
        if !approved_mask_set_matches_macos_build(&self.approved_mask_set, &self.macos_build) {
            return Err(GfmError::Format(format!(
                "parity manifest approved mask set `{}` must match macOS build `{}`",
                self.approved_mask_set, self.macos_build
            )));
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
        if self.appearance == ParityAppearance::System {
            return Err(GfmError::Format(
                "parity manifest appearance must be captured as resolved light or dark, not system"
                    .to_string(),
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
    pub provenance_path: PathBuf,
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
    run_parity_gate_at(inputs, current_utc_epoch_seconds()?)
}

fn run_parity_gate_at(
    inputs: Vec<ParityGateInput>,
    now_epoch_seconds: i64,
) -> Result<ParityGateReport> {
    let mut entries = Vec::with_capacity(inputs.len());
    for input in inputs {
        validate_distinct_capture_artifacts(&input)?;
        validate_baseline_not_expired(&input, now_epoch_seconds)?;
        validate_capture_provenance_artifacts(&input)?;
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

fn validate_baseline_not_expired(input: &ParityGateInput, now_epoch_seconds: i64) -> Result<()> {
    let Some(provenance) = &input.provenance else {
        return Ok(());
    };
    let Some(expires_at) = &provenance.expires_at else {
        return Ok(());
    };
    let expires_epoch_seconds = utc_capture_timestamp_epoch_seconds(expires_at)?;
    if now_epoch_seconds > expires_epoch_seconds {
        return Err(GfmError::Format(format!(
            "parity gate entry for {} uses expired Finder baseline: expires-at `{}` is before gate time `{}`",
            input.surface.as_str(),
            expires_at,
            utc_capture_timestamp_from_epoch_seconds(now_epoch_seconds)
        )));
    }
    Ok(())
}

fn validate_capture_provenance_artifacts(input: &ParityGateInput) -> Result<()> {
    let Some(provenance) = &input.provenance else {
        return Ok(());
    };
    provenance.validate()?;
    validate_mask_file_approval(input, provenance)?;
    let fixture_manifest = Path::new(&provenance.fixture_manifest);
    let missing_fixture_manifest = || {
        GfmError::Format(format!(
            "parity gate entry for {} requires captured fixture manifest file: {}",
            input.surface.as_str(),
            fixture_manifest.display()
        ))
    };
    let fixture_manifest_metadata = fs::metadata(fixture_manifest).map_err(|err| {
        if err.kind() == ErrorKind::NotFound {
            missing_fixture_manifest()
        } else {
            GfmError::io(fixture_manifest, err)
        }
    })?;
    if !fixture_manifest_metadata.is_file() {
        return Err(missing_fixture_manifest());
    }
    let fixture_manifest_content =
        fs::read_to_string(fixture_manifest).map_err(|err| GfmError::io(fixture_manifest, err))?;
    validate_fixture_manifest_contains_capture(input, fixture_manifest, &fixture_manifest_content)?;
    let missing_fixture_root = || {
        GfmError::Format(format!(
            "parity gate entry for {} requires captured fixture root directory: {}",
            input.surface.as_str(),
            provenance.fixture_root.display()
        ))
    };
    let fixture_root_metadata = fs::metadata(&provenance.fixture_root).map_err(|err| {
        if err.kind() == ErrorKind::NotFound {
            missing_fixture_root()
        } else {
            GfmError::io(&provenance.fixture_root, err)
        }
    })?;
    if !fixture_root_metadata.is_dir() {
        return Err(missing_fixture_root());
    }
    validate_capture_artifact_provenance(
        input,
        &input.expected_path,
        CaptureArtifactKind::Finder,
        provenance,
    )?;
    validate_capture_artifact_provenance(
        input,
        &input.actual_path,
        CaptureArtifactKind::Gfm,
        provenance,
    )?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureArtifactKind {
    Finder,
    Gfm,
}

impl CaptureArtifactKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Finder => "finder",
            Self::Gfm => "gfm",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Finder => "expected Finder",
            Self::Gfm => "actual GFM",
        }
    }
}

fn validate_capture_artifact_provenance(
    input: &ParityGateInput,
    artifact_path: &Path,
    kind: CaptureArtifactKind,
    provenance: &ParityCaptureProvenance,
) -> Result<()> {
    let provenance_path = capture_artifact_provenance_path(artifact_path);
    let content = fs::read_to_string(&provenance_path).map_err(|err| {
        if err.kind() == ErrorKind::NotFound {
            GfmError::Format(format!(
                "parity gate entry for {} requires {} capture provenance file: {}",
                input.surface.as_str(),
                kind.label(),
                provenance_path.display()
            ))
        } else {
            GfmError::io(&provenance_path, err)
        }
    })?;
    let artifact =
        parse_capture_artifact_provenance(&content, &provenance_path, input.surface, kind)?;
    artifact.validate_against(input, artifact_path, kind, provenance, &provenance_path)
}

fn capture_artifact_provenance_path(artifact_path: &Path) -> PathBuf {
    artifact_path.with_extension("provenance.tsv")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CaptureArtifactProvenance {
    target: String,
    fixture_root: PathBuf,
    output: PathBuf,
    scenario: String,
    view_mode: ParityViewMode,
    macos_build: String,
    hardware_profile: String,
    display_profile: String,
    app_version: String,
    captured_at: String,
    expires_at: Option<String>,
    capture_command: String,
    reviewer: String,
    signer: String,
    approved_mask_set: String,
    appearance: ParityAppearance,
    scale: DisplayScale,
    color_profile: ColorProfile,
    focus: ParityFocusState,
    window_region: CaptureArtifactRegion,
}

impl CaptureArtifactProvenance {
    fn validate_against(
        &self,
        input: &ParityGateInput,
        artifact_path: &Path,
        kind: CaptureArtifactKind,
        provenance: &ParityCaptureProvenance,
        provenance_path: &Path,
    ) -> Result<()> {
        self.expect_value(
            "target",
            &self.target,
            kind.as_str(),
            input,
            provenance_path,
        )?;
        self.expect_path(
            "output",
            &self.output,
            artifact_path,
            input,
            provenance_path,
        )?;
        self.expect_path(
            "fixture-root",
            &self.fixture_root,
            &provenance.fixture_root,
            input,
            provenance_path,
        )?;
        if !fixture_scenario_matches_surface(&self.scenario, input.surface) {
            return Err(capture_artifact_provenance_mismatch(
                input,
                provenance_path,
                "scenario",
                &self.scenario,
                "scenario matching parity surface",
            ));
        }
        self.expect_value(
            "view-mode",
            self.view_mode.as_str(),
            provenance.view_mode.as_str(),
            input,
            provenance_path,
        )?;
        self.expect_value(
            "macos-build",
            &self.macos_build,
            &provenance.macos_build,
            input,
            provenance_path,
        )?;
        self.expect_value(
            "hardware-profile",
            &self.hardware_profile,
            &provenance.hardware_profile,
            input,
            provenance_path,
        )?;
        self.expect_value(
            "display-profile",
            &self.display_profile,
            &provenance.display_profile,
            input,
            provenance_path,
        )?;
        self.expect_value(
            "app-version",
            &self.app_version,
            &provenance.app_version,
            input,
            provenance_path,
        )?;
        self.expect_value(
            "captured-at",
            &self.captured_at,
            &provenance.captured_at,
            input,
            provenance_path,
        )?;
        match (&self.expires_at, &provenance.expires_at) {
            (Some(actual), Some(expected)) => {
                self.expect_value("expires-at", actual, expected, input, provenance_path)?;
            }
            (None, Some(expected)) => {
                return Err(capture_artifact_provenance_mismatch(
                    input,
                    provenance_path,
                    "expires-at",
                    "",
                    expected,
                ));
            }
            (Some(actual), None) => {
                return Err(capture_artifact_provenance_mismatch(
                    input,
                    provenance_path,
                    "expires-at",
                    actual,
                    "",
                ));
            }
            (None, None) => {}
        }
        if !capture_command_matches(&provenance.capture_command, &self.capture_command, kind) {
            return Err(capture_artifact_provenance_mismatch(
                input,
                provenance_path,
                "capture-command",
                &self.capture_command,
                &provenance.capture_command,
            ));
        }
        self.expect_value(
            "reviewer",
            &self.reviewer,
            &provenance.reviewer,
            input,
            provenance_path,
        )?;
        self.expect_value(
            "signer",
            &self.signer,
            &provenance.signer,
            input,
            provenance_path,
        )?;
        self.expect_value(
            "approved-mask-set",
            &self.approved_mask_set,
            &provenance.approved_mask_set,
            input,
            provenance_path,
        )?;
        self.expect_value(
            "appearance",
            self.appearance.as_str(),
            provenance.appearance.as_str(),
            input,
            provenance_path,
        )?;
        self.expect_value(
            "scale",
            self.scale.as_str(),
            provenance.scale.as_str(),
            input,
            provenance_path,
        )?;
        self.expect_value(
            "color-profile",
            self.color_profile.as_str(),
            provenance.color_profile.as_str(),
            input,
            provenance_path,
        )?;
        self.expect_value(
            "focus",
            self.focus.as_str(),
            provenance.focus.as_str(),
            input,
            provenance_path,
        )?;
        if self.window_region.width != provenance.window_size.width
            || self.window_region.height != provenance.window_size.height
        {
            return Err(capture_artifact_provenance_mismatch(
                input,
                provenance_path,
                "window-region",
                &self.window_region.as_tsv_value(),
                &format!(
                    "*,*,{},{}",
                    provenance.window_size.width, provenance.window_size.height
                ),
            ));
        }
        Ok(())
    }

    fn expect_value(
        &self,
        field: &str,
        actual: &str,
        expected: &str,
        input: &ParityGateInput,
        provenance_path: &Path,
    ) -> Result<()> {
        if actual == expected {
            return Ok(());
        }
        Err(capture_artifact_provenance_mismatch(
            input,
            provenance_path,
            field,
            actual,
            expected,
        ))
    }

    fn expect_path(
        &self,
        field: &str,
        actual: &Path,
        expected: &Path,
        input: &ParityGateInput,
        provenance_path: &Path,
    ) -> Result<()> {
        if paths_refer_to_same_capture_root(actual, expected) {
            return Ok(());
        }
        Err(capture_artifact_provenance_mismatch(
            input,
            provenance_path,
            field,
            &actual.display().to_string(),
            &expected.display().to_string(),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CaptureArtifactRegion {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl CaptureArtifactRegion {
    fn as_tsv_value(self) -> String {
        format!("{},{},{},{}", self.x, self.y, self.width, self.height)
    }
}

fn parse_capture_artifact_provenance(
    content: &str,
    provenance_path: &Path,
    surface: ParitySurface,
    kind: CaptureArtifactKind,
) -> Result<CaptureArtifactProvenance> {
    let fields = parse_capture_artifact_provenance_fields(content, provenance_path)?;
    Ok(CaptureArtifactProvenance {
        target: required_capture_provenance_field(&fields, "target", provenance_path)?.to_string(),
        fixture_root: PathBuf::from(required_capture_provenance_field(
            &fields,
            "fixture-root",
            provenance_path,
        )?),
        output: PathBuf::from(required_capture_provenance_field(
            &fields,
            "output",
            provenance_path,
        )?),
        scenario: required_capture_provenance_field(&fields, "scenario", provenance_path)?
            .to_string(),
        view_mode: required_capture_provenance_field(&fields, "view-mode", provenance_path)?
            .parse::<ParityViewMode>()
            .map_err(|err| {
                capture_artifact_provenance_parse_error(surface, kind, provenance_path, err)
            })?,
        macos_build: required_capture_provenance_field(&fields, "macos-build", provenance_path)?
            .to_string(),
        hardware_profile: required_capture_provenance_field(
            &fields,
            "hardware-profile",
            provenance_path,
        )?
        .to_string(),
        display_profile: required_capture_provenance_field(
            &fields,
            "display-profile",
            provenance_path,
        )?
        .to_string(),
        app_version: required_capture_provenance_field(&fields, "app-version", provenance_path)?
            .to_string(),
        captured_at: required_capture_provenance_field(&fields, "captured-at", provenance_path)?
            .to_string(),
        expires_at: optional_capture_provenance_field(&fields, "expires-at")
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        capture_command: required_capture_provenance_field(
            &fields,
            "capture-command",
            provenance_path,
        )?
        .to_string(),
        reviewer: required_capture_provenance_field(&fields, "reviewer", provenance_path)?
            .to_string(),
        signer: required_capture_provenance_field(&fields, "signer", provenance_path)?.to_string(),
        approved_mask_set: required_capture_provenance_field(
            &fields,
            "approved-mask-set",
            provenance_path,
        )?
        .to_string(),
        appearance: required_capture_provenance_field(&fields, "appearance", provenance_path)?
            .parse::<ParityAppearance>()
            .map_err(|err| {
                capture_artifact_provenance_parse_error(surface, kind, provenance_path, err)
            })?,
        scale: required_capture_provenance_field(&fields, "scale", provenance_path)?
            .parse::<DisplayScale>()
            .map_err(|err| {
                capture_artifact_provenance_parse_error(surface, kind, provenance_path, err)
            })?,
        color_profile: required_capture_provenance_field(
            &fields,
            "color-profile",
            provenance_path,
        )?
        .parse::<ColorProfile>()
        .map_err(|err| {
            capture_artifact_provenance_parse_error(surface, kind, provenance_path, err)
        })?,
        focus: required_capture_provenance_field(&fields, "focus", provenance_path)?
            .parse::<ParityFocusState>()
            .map_err(|err| {
                capture_artifact_provenance_parse_error(surface, kind, provenance_path, err)
            })?,
        window_region: parse_capture_artifact_region(
            required_capture_provenance_field(&fields, "window-region", provenance_path)?,
            surface,
            kind,
            provenance_path,
        )?,
    })
}

fn parse_capture_artifact_provenance_fields(
    content: &str,
    provenance_path: &Path,
) -> Result<BTreeMap<String, String>> {
    let mut fields = BTreeMap::new();
    for (line_index, line) in content.lines().enumerate() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let columns = line.split('\t').collect::<Vec<_>>();
        if columns.len() != 2 {
            return Err(GfmError::Format(format!(
                "capture provenance file {} line {} must contain exactly key and value columns",
                provenance_path.display(),
                line_index + 1
            )));
        }
        let key = columns[0].trim();
        if key.is_empty() {
            return Err(GfmError::Format(format!(
                "capture provenance file {} line {} has empty key",
                provenance_path.display(),
                line_index + 1
            )));
        }
        if fields
            .insert(
                key.to_string(),
                unescape_capture_tsv_field(columns[1], provenance_path, line_index)?,
            )
            .is_some()
        {
            return Err(GfmError::Format(format!(
                "capture provenance file {} line {} duplicates `{key}`",
                provenance_path.display(),
                line_index + 1
            )));
        }
    }
    Ok(fields)
}

fn required_capture_provenance_field<'a>(
    fields: &'a BTreeMap<String, String>,
    key: &str,
    provenance_path: &Path,
) -> Result<&'a str> {
    fields.get(key).map(|value| value.as_str()).ok_or_else(|| {
        GfmError::Format(format!(
            "capture provenance file {} missing `{key}`",
            provenance_path.display()
        ))
    })
}

fn optional_capture_provenance_field<'a>(
    fields: &'a BTreeMap<String, String>,
    key: &str,
) -> Option<&'a str> {
    fields.get(key).map(|value| value.as_str())
}

fn unescape_capture_tsv_field(
    value: &str,
    provenance_path: &Path,
    line_index: usize,
) -> Result<String> {
    let mut output = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        let Some(escaped) = chars.next() else {
            return Err(GfmError::Format(format!(
                "capture provenance file {} line {} ends with an incomplete escape",
                provenance_path.display(),
                line_index + 1
            )));
        };
        match escaped {
            '\\' => output.push('\\'),
            't' => output.push('\t'),
            'n' => output.push('\n'),
            'r' => output.push('\r'),
            _ => {
                return Err(GfmError::Format(format!(
                    "capture provenance file {} line {} has unsupported escape \\{}",
                    provenance_path.display(),
                    line_index + 1,
                    escaped
                )))
            }
        }
    }
    Ok(output)
}

fn parse_capture_artifact_region(
    value: &str,
    surface: ParitySurface,
    kind: CaptureArtifactKind,
    provenance_path: &Path,
) -> Result<CaptureArtifactRegion> {
    let fields = value.split(',').collect::<Vec<_>>();
    if fields.len() != 4 {
        return Err(capture_artifact_provenance_parse_error(
            surface,
            kind,
            provenance_path,
            format!("invalid window-region `{value}`"),
        ));
    }
    Ok(CaptureArtifactRegion {
        x: parse_capture_region_u32(fields[0], "x", surface, kind, provenance_path)?,
        y: parse_capture_region_u32(fields[1], "y", surface, kind, provenance_path)?,
        width: parse_capture_region_u32(fields[2], "width", surface, kind, provenance_path)?,
        height: parse_capture_region_u32(fields[3], "height", surface, kind, provenance_path)?,
    })
}

fn parse_capture_region_u32(
    value: &str,
    field: &str,
    surface: ParitySurface,
    kind: CaptureArtifactKind,
    provenance_path: &Path,
) -> Result<u32> {
    value.parse::<u32>().map_err(|_| {
        capture_artifact_provenance_parse_error(
            surface,
            kind,
            provenance_path,
            format!("invalid window-region {field} `{value}`"),
        )
    })
}

fn capture_command_matches(
    manifest_command: &str,
    artifact_command: &str,
    kind: CaptureArtifactKind,
) -> bool {
    if artifact_command == manifest_command {
        return true;
    }
    let target_prefix = format!("{}:", kind.as_str());
    for command in manifest_command.split(';') {
        if let Some(command) = command.strip_prefix(&target_prefix) {
            return artifact_command == command;
        }
    }
    artifact_command
        .strip_prefix(manifest_command)
        .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with(':'))
}

fn capture_artifact_provenance_parse_error(
    surface: ParitySurface,
    kind: CaptureArtifactKind,
    provenance_path: &Path,
    message: impl std::fmt::Display,
) -> GfmError {
    GfmError::Format(format!(
        "parity gate entry for {} has invalid {} capture provenance file {}: {}",
        surface.as_str(),
        kind.label(),
        provenance_path.display(),
        message
    ))
}

fn capture_artifact_provenance_mismatch(
    input: &ParityGateInput,
    provenance_path: &Path,
    field: &str,
    actual: &str,
    expected: &str,
) -> GfmError {
    GfmError::Format(format!(
        "parity gate entry for {} has capture provenance mismatch in {} field `{}`: got `{}` expected `{}`",
        input.surface.as_str(),
        provenance_path.display(),
        field,
        actual,
        expected
    ))
}

fn validate_mask_file_approval(
    input: &ParityGateInput,
    provenance: &ParityCaptureProvenance,
) -> Result<()> {
    let Some(mask_path) = &input.mask_path else {
        return Ok(());
    };
    let content = fs::read_to_string(mask_path).map_err(|err| GfmError::io(mask_path, err))?;
    let approved_mask_set = governed_mask_file_approved_set(&content, mask_path)?;
    if approved_mask_set != provenance.approved_mask_set {
        return Err(GfmError::Format(format!(
            "parity gate entry for {} uses governed mask file {} approved for `{}` but manifest approved-mask-set is `{}`",
            input.surface.as_str(),
            mask_path.display(),
            approved_mask_set,
            provenance.approved_mask_set
        )));
    }
    Ok(())
}

fn governed_mask_file_approved_set(content: &str, mask_path: &Path) -> Result<String> {
    let mut approved_mask_set = None;
    for (line_index, line) in content.lines().enumerate() {
        let line = line.trim();
        if !line.starts_with('#') {
            continue;
        }
        let directive = line.trim_start_matches('#').trim();
        let Some(value) = directive.strip_prefix("approved-mask-set=") else {
            continue;
        };
        if approved_mask_set.is_some() {
            return Err(GfmError::Format(format!(
                "governed mask file {} line {} duplicates approved-mask-set",
                mask_path.display(),
                line_index + 1
            )));
        }
        let value = value.trim();
        if value.is_empty() {
            return Err(GfmError::Format(format!(
                "governed mask file {} line {} has empty approved-mask-set",
                mask_path.display(),
                line_index + 1
            )));
        }
        approved_mask_set = Some(value.to_string());
    }
    approved_mask_set.ok_or_else(|| {
        GfmError::Format(format!(
            "parity gate requires governed mask file {} to declare # approved-mask-set=<id>",
            mask_path.display()
        ))
    })
}

fn validate_fixture_manifest_contains_capture(
    input: &ParityGateInput,
    fixture_manifest: &Path,
    content: &str,
) -> Result<()> {
    let Some(provenance) = &input.provenance else {
        return Ok(());
    };
    let base = fixture_manifest.parent().unwrap_or_else(|| Path::new(""));
    for line in content.lines() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') || line.starts_with("scenario\t") {
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() < 3 {
            continue;
        }
        let root = resolve_manifest_path(base, fields[1]);
        if paths_refer_to_same_capture_root(&root, &provenance.fixture_root)
            && fields[2] == provenance.view_mode.as_str()
            && fixture_scenario_matches_surface(fields[0], input.surface)
        {
            return Ok(());
        }
    }
    Err(GfmError::Format(format!(
        "parity gate entry for {} requires captured fixture manifest {} to reference fixture root {} with view mode {} and matching scenario",
        input.surface.as_str(),
        fixture_manifest.display(),
        provenance.fixture_root.display(),
        provenance.view_mode.as_str()
    )))
}

fn fixture_scenario_matches_surface(scenario: &str, surface: ParitySurface) -> bool {
    match surface {
        ParitySurface::Sidebar => scenario == "sidebar",
        ParitySurface::Selection => scenario == "selection",
        ParitySurface::Toolbar => scenario == "toolbar",
        ParitySurface::Sheet => matches!(scenario, "sheet" | "conflict-sheet"),
        ParitySurface::Menu => scenario == "menu",
        ParitySurface::Layout
        | ParitySurface::Text
        | ParitySurface::Icon
        | ParitySurface::Focus
        | ParitySurface::Hover
        | ParitySurface::Thumbnail
        | ParitySurface::Preview => true,
    }
}

fn paths_refer_to_same_capture_root(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
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
    let provenance_path = output_dir.join("provenance.tsv");
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
    write_text(&provenance_path, &render_provenance_tsv(&report))?;
    let manifest_context = BundleManifestContext {
        review_path: &review_path,
        entries_path: &entries_path,
        violations_path: &violations_path,
        first_mismatch_path: &first_mismatch_path,
        region_summary_path: &region_summary_path,
        mask_justification_path: &mask_justification_path,
        provenance_path: &provenance_path,
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
        provenance_path,
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
        fixture_manifest: resolve_manifest_path(base, &profile.fixture_manifest)
            .display()
            .to_string(),
        captured_at: profile.captured_at.clone(),
        expires_at: profile.expires_at.clone(),
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
    expires_at: Option<String>,
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
    let mut expires_at = None;
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
            "expires-at" => expires_at = Some(value.to_string()),
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
        expires_at,
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
    if !is_valid_utc_capture_timestamp(&profile.captured_at) {
        return Err(GfmError::Format(format!(
            "parity gate manifest line {} has invalid captured-at `{}`; expected UTC second precision like 2026-08-27T00:00:00Z",
            line_index + 1,
            profile.captured_at
        )));
    }
    if let Some(expires_at) = &profile.expires_at {
        if !is_valid_utc_capture_timestamp(expires_at) {
            return Err(GfmError::Format(format!(
                "parity gate manifest line {} has invalid expires-at `{}`; expected UTC second precision like 2026-09-27T00:00:00Z",
                line_index + 1,
                expires_at
            )));
        }
        if utc_capture_timestamp_epoch_seconds(&profile.captured_at)?
            > utc_capture_timestamp_epoch_seconds(expires_at)?
        {
            return Err(GfmError::Format(format!(
                "parity gate manifest line {} has stale baseline captured-at `{}` after expires-at `{}`",
                line_index + 1,
                profile.captured_at,
                expires_at
            )));
        }
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
    if !approved_mask_set_matches_macos_build(&profile.approved_mask_set, &profile.macos_build) {
        return Err(GfmError::Format(format!(
            "parity gate manifest line {} has approved-mask-set `{}` that does not match macos-build `{}`",
            line_index + 1,
            profile.approved_mask_set,
            profile.macos_build
        )));
    }
    Ok(profile)
}

fn approved_mask_set_matches_macos_build(mask_set: &str, macos_build: &str) -> bool {
    mask_set
        .split(|ch: char| !(ch.is_ascii_alphanumeric()))
        .any(|token| token == macos_build)
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

    let max_day = days_in_month(year, month);
    (1..=max_day).contains(&day)
}

fn utc_capture_timestamp_epoch_seconds(value: &str) -> Result<i64> {
    if !is_valid_utc_capture_timestamp(value) {
        return Err(GfmError::Format(format!(
            "invalid UTC capture timestamp `{value}`"
        )));
    }
    let bytes = value.as_bytes();
    let year = i64::from(parse_fixed_u32(bytes, 0, 4));
    let month = i64::from(parse_fixed_u32(bytes, 5, 7));
    let day = i64::from(parse_fixed_u32(bytes, 8, 10));
    let hour = i64::from(parse_fixed_u32(bytes, 11, 13));
    let minute = i64::from(parse_fixed_u32(bytes, 14, 16));
    let second = i64::from(parse_fixed_u32(bytes, 17, 19));
    let days = days_from_civil(year, month, day);
    Ok(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn current_utc_epoch_seconds() -> Result<i64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| {
            GfmError::Format(format!(
            "system clock is before the Unix epoch; cannot validate parity baseline expiry: {err}"
        ))
        })?;
    i64::try_from(duration.as_secs()).map_err(|_| {
        GfmError::Format("system clock value is too large for parity baseline expiry".to_string())
    })
}

fn utc_capture_timestamp_from_epoch_seconds(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let second_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = second_of_day / 3_600;
    let minute = (second_of_day % 3_600) / 60;
    let second = second_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
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

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

fn parse_fixed_u32(bytes: &[u8], start: usize, end: usize) -> u32 {
    bytes[start..end]
        .iter()
        .fold(0, |value, byte| value * 10 + u32::from(byte - b'0'))
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

fn render_review_markdown(report: &ParityGateReport) -> String {
    let manifest = report
        .manifest_path
        .as_ref()
        .map(|path| escape_markdown_inline_code_path(path))
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
        text.push_str("| Surface | macOS Build | Appearance | Scale | Color Profile | Window | Focus | View Mode | Fixture Root | Captured | Expires | Reviewer | Signer | Approved Masks |\n");
        text.push_str(
            "| --- | --- | --- | --- | --- | ---: | --- | --- | --- | --- | --- | --- | --- | --- |\n",
        );
        for entry in &report.entries {
            if let Some(provenance) = &entry.input.provenance {
                text.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {}x{} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                    entry.input.surface.as_str(),
                    escape_markdown_table_cell(&provenance.macos_build),
                    provenance.appearance.as_str(),
                    provenance.scale.as_str(),
                    provenance.color_profile.as_str(),
                    provenance.window_size.width,
                    provenance.window_size.height,
                    provenance.focus.as_str(),
                    provenance.view_mode.as_str(),
                    escape_markdown_table_cell(&provenance.fixture_root.display().to_string()),
                    escape_markdown_table_cell(&provenance.captured_at),
                    escape_markdown_table_cell(provenance.expires_at.as_deref().unwrap_or("")),
                    escape_markdown_table_cell(&provenance.reviewer),
                    escape_markdown_table_cell(&provenance.signer),
                    escape_markdown_table_cell(&provenance.approved_mask_set)
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
        "surface\twidth\theight\texpected\tactual\tmask\tmacos-build\thardware-profile\tdisplay-profile\tapp-version\tfixture-manifest\tcaptured-at\texpires-at\tcapture-command\treviewer\tsigner\tapproved-mask-set\tappearance\tscale\tcolor-profile\twindow-width\twindow-height\tfocus\tview-mode\tfixture-root\tmismatched\tunmasked\tmasked\tmax-channel-delta\tpassed\n"
            .to_string();
    for entry in &report.entries {
        let provenance = entry.input.provenance.as_ref();
        text.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            entry.input.surface.as_str(),
            entry.diff.size.width,
            entry.diff.size.height,
            escape_tsv_path(&entry.input.expected_path),
            escape_tsv_path(&entry.input.actual_path),
            entry
                .input
                .mask_path
                .as_ref()
                .map(|path| escape_tsv_path(path))
                .unwrap_or_default(),
            provenance
                .map(|value| value.macos_build.as_str())
                .map(escape_tsv_field)
                .unwrap_or_default(),
            provenance
                .map(|value| value.hardware_profile.as_str())
                .map(escape_tsv_field)
                .unwrap_or_default(),
            provenance
                .map(|value| value.display_profile.as_str())
                .map(escape_tsv_field)
                .unwrap_or_default(),
            provenance
                .map(|value| value.app_version.as_str())
                .map(escape_tsv_field)
                .unwrap_or_default(),
            provenance
                .map(|value| value.fixture_manifest.as_str())
                .map(escape_tsv_field)
                .unwrap_or_default(),
            provenance
                .map(|value| value.captured_at.as_str())
                .map(escape_tsv_field)
                .unwrap_or_default(),
            provenance
                .and_then(|value| value.expires_at.as_deref())
                .map(escape_tsv_field)
                .unwrap_or_default(),
            provenance
                .map(|value| value.capture_command.as_str())
                .map(escape_tsv_field)
                .unwrap_or_default(),
            provenance
                .map(|value| value.reviewer.as_str())
                .map(escape_tsv_field)
                .unwrap_or_default(),
            provenance
                .map(|value| value.signer.as_str())
                .map(escape_tsv_field)
                .unwrap_or_default(),
            provenance
                .map(|value| value.approved_mask_set.as_str())
                .map(escape_tsv_field)
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
                .map(|value| escape_tsv_field(&value))
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

fn render_provenance_tsv(report: &ParityGateReport) -> String {
    let mut text =
        "surface\tmacos-build\thardware-profile\tdisplay-profile\tapp-version\tfixture-manifest\tcaptured-at\texpires-at\tcapture-command\treviewer\tsigner\tapproved-mask-set\tappearance\tscale\tcolor-profile\twindow-width\twindow-height\tfocus\tview-mode\tfixture-root\n"
            .to_string();
    for entry in &report.entries {
        if let Some(provenance) = &entry.input.provenance {
            text.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                entry.input.surface.as_str(),
                escape_tsv_field(&provenance.macos_build),
                escape_tsv_field(&provenance.hardware_profile),
                escape_tsv_field(&provenance.display_profile),
                escape_tsv_field(&provenance.app_version),
                escape_tsv_field(&provenance.fixture_manifest),
                escape_tsv_field(&provenance.captured_at),
                provenance
                    .expires_at
                    .as_deref()
                    .map(escape_tsv_field)
                    .unwrap_or_default(),
                escape_tsv_field(&provenance.capture_command),
                escape_tsv_field(&provenance.reviewer),
                escape_tsv_field(&provenance.signer),
                escape_tsv_field(&provenance.approved_mask_set),
                provenance.appearance.as_str(),
                provenance.scale.as_str(),
                provenance.color_profile.as_str(),
                provenance.window_size.width,
                provenance.window_size.height,
                provenance.focus.as_str(),
                provenance.view_mode.as_str(),
                escape_tsv_path(&provenance.fixture_root)
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
    provenance_path: &'a Path,
    visual_diff_dir: &'a Path,
    source_artifact_dir: &'a Path,
    artifact_rows: &'a [String],
}

fn render_bundle_manifest(context: &BundleManifestContext<'_>) -> String {
    let mut text = "kind\tpath\tbytes\tfnv1a64\n".to_string();
    for (kind, path) in [
        ("review", context.review_path),
        ("entries", context.entries_path),
        ("violations", context.violations_path),
        ("first-unmasked", context.first_mismatch_path),
        ("regions", context.region_summary_path),
        ("mask-justifications", context.mask_justification_path),
        ("provenance", context.provenance_path),
        ("visual-diffs", context.visual_diff_dir),
        ("source-artifacts", context.source_artifact_dir),
    ] {
        text.push_str(&render_bundle_manifest_row(kind, path));
    }
    text.push_str(&context.artifact_rows.join(""));
    text
}

fn render_bundle_manifest_row(kind: &str, path: &Path) -> String {
    let (bytes, hash) = match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            let bytes = metadata.len();
            let hash = fs::read(path)
                .map(|content| format!("{:016x}", fnv1a64(&content)))
                .unwrap_or_else(|_| "-".to_string());
            (bytes.to_string(), hash)
        }
        _ => ("-".to_string(), "-".to_string()),
    };
    format!("{}\t{}\t{}\t{}\n", kind, escape_tsv_path(path), bytes, hash)
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
    })
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
        rows.push(render_bundle_manifest_row("visual-diff", &diff_path));

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
        rows.push(render_bundle_manifest_row("finder-source", &expected_copy));
        rows.push(render_bundle_manifest_row("gfm-source", &actual_copy));
        if entry.input.provenance.is_some() {
            let provenance_copy = copy_capture_provenance_artifact(
                &entry.input,
                &entry.input.expected_path,
                &source_artifact_dir.join(format!("{stem}-finder.provenance.tsv")),
                CaptureArtifactKind::Finder,
            )?;
            rows.push(render_bundle_manifest_row(
                "finder-provenance",
                &provenance_copy,
            ));
            let provenance_copy = copy_capture_provenance_artifact(
                &entry.input,
                &entry.input.actual_path,
                &source_artifact_dir.join(format!("{stem}-gfm.provenance.tsv")),
                CaptureArtifactKind::Gfm,
            )?;
            rows.push(render_bundle_manifest_row(
                "gfm-provenance",
                &provenance_copy,
            ));
        }
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

fn copy_capture_provenance_artifact(
    input: &ParityGateInput,
    source_artifact: &Path,
    destination: &Path,
    kind: CaptureArtifactKind,
) -> Result<PathBuf> {
    let provenance = capture_artifact_provenance_path(source_artifact);
    match fs::copy(&provenance, destination) {
        Ok(_) => Ok(destination.to_path_buf()),
        Err(err) if err.kind() == ErrorKind::NotFound => Err(GfmError::Format(format!(
            "parity review bundle for {} requires {} capture provenance file: {}",
            input.surface.as_str(),
            kind.label(),
            provenance.display()
        ))),
        Err(err) => Err(GfmError::io(&provenance, err)),
    }
}

fn escape_tsv_field(value: &str) -> String {
    value.replace(['\t', '\n', '\r'], " ")
}

fn escape_tsv_path(path: &Path) -> String {
    escape_tsv_field(&path.display().to_string())
}

fn escape_markdown_table_cell(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace(['\t', '\n', '\r'], " ")
}

fn escape_markdown_inline_code_path(path: &Path) -> String {
    path.display()
        .to_string()
        .replace('`', "\\`")
        .replace(['\t', '\n', '\r'], " ")
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
        fs::write(&mask, "1\t0\t1\t1\tOS-owned system menu clock\n").unwrap();

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
        write_capture_provenance_artifacts(&root, "fixtures/icon");
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
    fn parity_gate_rejects_missing_fixture_manifest_provenance_artifact() {
        let root = unique_temp_dir("gfm-parity-gate-missing-fixture-manifest");
        fs::write(root.join("expected.rgba"), [1, 2, 3, 255]).unwrap();
        fs::write(root.join("actual.rgba"), [1, 2, 3, 255]).unwrap();
        fs::create_dir_all(root.join("fixtures/icon")).unwrap();
        fs::write(
            root.join("gate.tsv"),
            "manifest-version\t1\nprofile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=light\tscale=2x\tcolor-profile=srgb\nentry\ticon\texpected.rgba\tactual.rgba\t1\t1\t\t1040\t720\tactive\ticon\tfixtures/icon\n",
        )
        .unwrap();

        let err = run_parity_gate_manifest(root.join("gate.tsv")).unwrap_err();

        assert!(err
            .to_string()
            .contains("requires captured fixture manifest file"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parity_gate_rejects_missing_fixture_root_provenance_artifact() {
        let root = unique_temp_dir("gfm-parity-gate-missing-fixture-root");
        fs::write(root.join("expected.rgba"), [1, 2, 3, 255]).unwrap();
        fs::write(root.join("actual.rgba"), [1, 2, 3, 255]).unwrap();
        fs::create_dir_all(root.join("fixtures")).unwrap();
        fs::write(
            root.join("fixtures/manifest.tsv"),
            format!(
                "scenario\troot\tfinder-view\tfiles\tdirectories\nicon\t{}\ticon\t1\t0\n",
                root.join("fixtures/icon").display()
            ),
        )
        .unwrap();
        fs::write(
            root.join("gate.tsv"),
            "manifest-version\t1\nprofile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=light\tscale=2x\tcolor-profile=srgb\nentry\ticon\texpected.rgba\tactual.rgba\t1\t1\t\t1040\t720\tactive\ticon\tfixtures/icon\n",
        )
        .unwrap();

        let err = run_parity_gate_manifest(root.join("gate.tsv")).unwrap_err();

        assert!(err
            .to_string()
            .contains("requires captured fixture root directory"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parity_gate_rejects_fixture_root_missing_from_fixture_manifest() {
        let root = unique_temp_dir("gfm-parity-gate-mismatched-fixture-manifest-root");
        fs::write(root.join("expected.rgba"), [1, 2, 3, 255]).unwrap();
        fs::write(root.join("actual.rgba"), [1, 2, 3, 255]).unwrap();
        fs::create_dir_all(root.join("fixtures/icon")).unwrap();
        fs::write(
            root.join("fixtures/manifest.tsv"),
            format!(
                "scenario\troot\tfinder-view\tfiles\tdirectories\nlist\t{}\tlist\t1\t0\n",
                root.join("fixtures/list").display()
            ),
        )
        .unwrap();
        fs::write(
            root.join("gate.tsv"),
            "manifest-version\t1\nprofile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=light\tscale=2x\tcolor-profile=srgb\nentry\ticon\texpected.rgba\tactual.rgba\t1\t1\t\t1040\t720\tactive\ticon\tfixtures/icon\n",
        )
        .unwrap();

        let err = run_parity_gate_manifest(root.join("gate.tsv")).unwrap_err();

        assert!(err
            .to_string()
            .contains("requires captured fixture manifest"));
        assert!(err.to_string().contains("to reference fixture root"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parity_gate_rejects_surface_specific_capture_from_wrong_fixture_scenario() {
        let root = unique_temp_dir("gfm-parity-gate-wrong-fixture-scenario");
        fs::write(root.join("expected.rgba"), [1, 2, 3, 255]).unwrap();
        fs::write(root.join("actual.rgba"), [1, 2, 3, 255]).unwrap();
        fs::create_dir_all(root.join("fixtures/icon")).unwrap();
        fs::write(
            root.join("fixtures/manifest.tsv"),
            format!(
                "scenario\troot\tfinder-view\tfiles\tdirectories\nicon\t{}\ticon\t1\t0\n",
                root.join("fixtures/icon").display()
            ),
        )
        .unwrap();
        fs::write(
            root.join("gate.tsv"),
            "manifest-version\t1\nprofile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=light\tscale=2x\tcolor-profile=srgb\nentry\ttoolbar\texpected.rgba\tactual.rgba\t1\t1\t\t1040\t720\tactive\ticon\tfixtures/icon\n",
        )
        .unwrap();

        let err = run_parity_gate_manifest(root.join("gate.tsv")).unwrap_err();

        assert!(err
            .to_string()
            .contains("with view mode icon and matching scenario"));
        assert!(err.to_string().contains("toolbar"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parity_gate_rejects_missing_capture_artifact_provenance() {
        let root = unique_temp_dir("gfm-parity-gate-missing-artifact-provenance");
        fs::write(root.join("expected.rgba"), [1, 2, 3, 255]).unwrap();
        fs::write(root.join("actual.rgba"), [1, 2, 3, 255]).unwrap();
        write_capture_provenance_artifacts(&root, "fixtures/icon");
        fs::remove_file(root.join("expected.provenance.tsv")).unwrap();
        fs::write(
            root.join("gate.tsv"),
            "manifest-version\t1\nprofile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=light\tscale=2x\tcolor-profile=srgb\nentry\ticon\texpected.rgba\tactual.rgba\t1\t1\t\t1040\t720\tactive\ticon\tfixtures/icon\n",
        )
        .unwrap();

        let err = run_parity_gate_manifest(root.join("gate.tsv")).unwrap_err();

        assert!(err
            .to_string()
            .contains("requires expected Finder capture provenance file"));
        assert!(err.to_string().contains("expected.provenance.tsv"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parity_gate_rejects_capture_provenance_output_mismatch() {
        let root = unique_temp_dir("gfm-parity-gate-artifact-output-mismatch");
        fs::write(root.join("expected.rgba"), [1, 2, 3, 255]).unwrap();
        fs::write(root.join("actual.rgba"), [1, 2, 3, 255]).unwrap();
        write_capture_provenance_artifacts(&root, "fixtures/icon");
        let provenance = fs::read_to_string(root.join("actual.provenance.tsv"))
            .unwrap()
            .replace("actual.rgba", "wrong.rgba");
        fs::write(root.join("actual.provenance.tsv"), provenance).unwrap();
        fs::write(
            root.join("gate.tsv"),
            "manifest-version\t1\nprofile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=light\tscale=2x\tcolor-profile=srgb\nentry\ticon\texpected.rgba\tactual.rgba\t1\t1\t\t1040\t720\tactive\ticon\tfixtures/icon\n",
        )
        .unwrap();

        let err = run_parity_gate_manifest(root.join("gate.tsv")).unwrap_err();

        assert!(err.to_string().contains("actual.provenance.tsv"));
        assert!(err.to_string().contains("capture provenance mismatch"));
        assert!(err.to_string().contains("field `output`"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parity_gate_rejects_missing_artifact_expiry_when_manifest_expires() {
        let root = unique_temp_dir("gfm-parity-gate-artifact-missing-expiry");
        fs::write(root.join("expected.rgba"), [1, 2, 3, 255]).unwrap();
        fs::write(root.join("actual.rgba"), [1, 2, 3, 255]).unwrap();
        write_capture_provenance_artifacts(&root, "fixtures/toolbar");
        fs::write(
            root.join("gate.tsv"),
            "manifest-version\t1\nprofile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\texpires-at=2999-01-01T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=light\tscale=2x\tcolor-profile=srgb\nentry\ttoolbar\texpected.rgba\tactual.rgba\t1\t1\t\t1040\t720\tactive\ticon\tfixtures/toolbar\n",
        )
        .unwrap();

        let err = run_parity_gate_manifest(root.join("gate.tsv")).unwrap_err();

        assert!(err.to_string().contains("expected.provenance.tsv"));
        assert!(err.to_string().contains("capture provenance mismatch"));
        assert!(err.to_string().contains("field `expires-at`"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parity_gate_rejects_provenance_mask_without_approved_set() {
        let root = unique_temp_dir("gfm-parity-gate-mask-missing-approval");
        fs::write(root.join("expected.rgba"), [1, 2, 3, 255]).unwrap();
        fs::write(root.join("actual.rgba"), [1, 2, 2, 255]).unwrap();
        fs::write(
            root.join("mask.tsv"),
            "0\t0\t1\t1\tOS-owned toolbar repaint\n",
        )
        .unwrap();
        write_capture_provenance_artifacts(&root, "fixtures/toolbar");
        fs::write(
            root.join("gate.tsv"),
            "manifest-version\t1\nprofile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=light\tscale=2x\tcolor-profile=srgb\nentry\ttoolbar\texpected.rgba\tactual.rgba\t1\t1\tmask.tsv\t1040\t720\tactive\ticon\tfixtures/toolbar\n",
        )
        .unwrap();

        let err = run_parity_gate_manifest(root.join("gate.tsv")).unwrap_err();

        assert!(err.to_string().contains("requires governed mask file"));
        assert!(err.to_string().contains("approved-mask-set"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parity_gate_rejects_provenance_mask_approved_for_different_set() {
        let root = unique_temp_dir("gfm-parity-gate-mask-wrong-approval");
        fs::write(root.join("expected.rgba"), [1, 2, 3, 255]).unwrap();
        fs::write(root.join("actual.rgba"), [1, 2, 2, 255]).unwrap();
        fs::write(
            root.join("mask.tsv"),
            "# approved-mask-set=macos-25B999-default\n0\t0\t1\t1\tOS-owned toolbar repaint\n",
        )
        .unwrap();
        write_capture_provenance_artifacts(&root, "fixtures/toolbar");
        fs::write(
            root.join("gate.tsv"),
            "manifest-version\t1\nprofile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=light\tscale=2x\tcolor-profile=srgb\nentry\ttoolbar\texpected.rgba\tactual.rgba\t1\t1\tmask.tsv\t1040\t720\tactive\ticon\tfixtures/toolbar\n",
        )
        .unwrap();

        let err = run_parity_gate_manifest(root.join("gate.tsv")).unwrap_err();

        assert!(err.to_string().contains("macos-25B999-default"));
        assert!(err.to_string().contains("macos-25A354-default"));

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
        assert!(provenance
            .fixture_manifest
            .ends_with("fixtures/manifest.tsv"));
        assert_eq!(provenance.captured_at, "2026-08-27T00:00:00Z");
        assert_eq!(provenance.expires_at, None);
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
    fn versioned_parity_manifest_rejects_invalid_expiry_timestamp() {
        let root = unique_temp_dir("gfm-parity-gate-invalid-expiry");
        let err = parse_parity_gate_manifest(
            "manifest-version\t1\nprofile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\texpires-at=next-week\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=dark\tscale=2x\tcolor-profile=display-p3\nentry\ttoolbar\tfinder.png\tgfm.png\t1\t1\t\t1440\t900\tactive\ticon\tfixtures/icon\n",
            &root,
        )
        .unwrap_err();

        assert!(err.to_string().contains("invalid expires-at"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn versioned_parity_manifest_rejects_stale_expired_capture_profile() {
        let root = unique_temp_dir("gfm-parity-gate-stale-profile");
        let err = parse_parity_gate_manifest(
            "manifest-version\t1\nprofile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-09-10T00:00:00Z\texpires-at=2026-09-09T23:59:59Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=dark\tscale=2x\tcolor-profile=display-p3\nentry\ttoolbar\tfinder.png\tgfm.png\t1\t1\t\t1440\t900\tactive\ticon\tfixtures/icon\n",
            &root,
        )
        .unwrap_err();

        assert!(err.to_string().contains("stale baseline"));
        assert!(err.to_string().contains("expires-at"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parity_gate_rejects_expired_baseline_before_provenance_io() {
        let root = unique_temp_dir("gfm-parity-gate-expired-before-provenance");
        let expected = root.join("finder.rgba");
        let actual = root.join("gfm.rgba");
        fs::write(&expected, [0, 0, 0, 255]).unwrap();
        fs::write(&actual, [0, 0, 0, 255]).unwrap();
        let input = ParityGateInput::new(
            ParitySurface::Toolbar,
            &expected,
            &actual,
            PixelSize::new(1, 1),
        )
        .with_provenance(ParityCaptureProvenance {
            macos_build: "25A354".to_string(),
            hardware_profile: "macbookpro18,3".to_string(),
            display_profile: "studio-display-p3".to_string(),
            app_version: "0.1.0".to_string(),
            fixture_manifest: "fixtures/manifest.tsv".to_string(),
            captured_at: "2026-08-27T00:00:00Z".to_string(),
            expires_at: Some("2026-09-09T23:59:59Z".to_string()),
            capture_command: "screencapture:-x".to_string(),
            reviewer: "codex".to_string(),
            signer: "codex".to_string(),
            approved_mask_set: "macos-25A354-default".to_string(),
            appearance: ParityAppearance::Dark,
            scale: DisplayScale::Two,
            color_profile: ColorProfile::DisplayP3,
            window_size: PixelSize::new(1440, 900),
            focus: ParityFocusState::Active,
            view_mode: ParityViewMode::Icon,
            fixture_root: PathBuf::from("fixtures/icon"),
        });

        let err = run_parity_gate_at(
            vec![input],
            utc_capture_timestamp_epoch_seconds("2026-09-10T00:00:00Z").unwrap(),
        )
        .unwrap_err();

        assert!(err.to_string().contains("expired Finder baseline"));
        assert!(err
            .to_string()
            .contains("expires-at `2026-09-09T23:59:59Z`"));
        assert!(!err.to_string().contains("requires expected Finder capture"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parity_baseline_expiry_accepts_exact_expiry_second() {
        let input = ParityGateInput::new(
            ParitySurface::Toolbar,
            "finder.rgba",
            "gfm.rgba",
            PixelSize::new(1, 1),
        )
        .with_provenance(ParityCaptureProvenance {
            macos_build: "25A354".to_string(),
            hardware_profile: "macbookpro18,3".to_string(),
            display_profile: "studio-display-p3".to_string(),
            app_version: "0.1.0".to_string(),
            fixture_manifest: "fixtures/manifest.tsv".to_string(),
            captured_at: "2026-08-27T00:00:00Z".to_string(),
            expires_at: Some("2026-09-09T23:59:59Z".to_string()),
            capture_command: "screencapture:-x".to_string(),
            reviewer: "codex".to_string(),
            signer: "codex".to_string(),
            approved_mask_set: "macos-25A354-default".to_string(),
            appearance: ParityAppearance::Dark,
            scale: DisplayScale::Two,
            color_profile: ColorProfile::DisplayP3,
            window_size: PixelSize::new(1440, 900),
            focus: ParityFocusState::Active,
            view_mode: ParityViewMode::Icon,
            fixture_root: PathBuf::from("fixtures/icon"),
        });

        validate_baseline_not_expired(
            &input,
            utc_capture_timestamp_epoch_seconds("2026-09-09T23:59:59Z").unwrap(),
        )
        .unwrap();
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
    fn versioned_parity_manifest_rejects_invalid_capture_timestamp() {
        let root = unique_temp_dir("gfm-parity-gate-invalid-captured-at");
        let err = parse_parity_gate_manifest(
            "manifest-version\t1\nprofile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=next-week\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=dark\tscale=2x\tcolor-profile=display-p3\nentry\ttoolbar\tfinder.png\tgfm.png\t1\t1\t\t1440\t900\tactive\ticon\tfixtures/icon\n",
            &root,
        )
        .unwrap_err();

        assert!(err.to_string().contains("line 2"));
        assert!(err.to_string().contains("invalid captured-at `next-week`"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn versioned_parity_manifest_rejects_mismatched_approved_mask_build() {
        let root = unique_temp_dir("gfm-parity-gate-mismatched-mask-build");
        let err = parse_parity_gate_manifest(
            "manifest-version\t1\nprofile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25B999-default\tappearance=dark\tscale=2x\tcolor-profile=display-p3\nentry\ttoolbar\tfinder.png\tgfm.png\t1\t1\t\t1440\t900\tactive\ticon\tfixtures/icon\n",
            &root,
        )
        .unwrap_err();

        assert!(err.to_string().contains("line 2"));
        assert!(err
            .to_string()
            .contains("approved-mask-set `macos-25B999-default`"));
        assert!(err.to_string().contains("macos-build `25A354`"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn versioned_parity_manifest_rejects_unresolved_system_appearance() {
        let root = unique_temp_dir("gfm-parity-gate-system-appearance");
        let err = parse_parity_gate_manifest(
            "manifest-version\t1\nprofile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2026-08-27T00:00:00Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=system\tscale=2x\tcolor-profile=display-p3\nentry\ttoolbar\tfinder.png\tgfm.png\t1\t1\t\t1440\t900\tactive\ticon\tfixtures/icon\n",
            &root,
        )
        .unwrap_err();

        assert!(err.to_string().contains("resolved light or dark"));
        assert!(err.to_string().contains("not system"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parity_capture_provenance_rejects_mismatched_approved_mask_build() {
        let provenance = ParityCaptureProvenance {
            macos_build: "25A354".to_string(),
            hardware_profile: "macbookpro18,3".to_string(),
            display_profile: "studio-display-p3".to_string(),
            app_version: "0.1.0".to_string(),
            fixture_manifest: "fixtures/manifest.tsv".to_string(),
            captured_at: "2026-08-27T00:00:00Z".to_string(),
            expires_at: None,
            capture_command: "screencapture:-x".to_string(),
            reviewer: "codex".to_string(),
            signer: "codex".to_string(),
            approved_mask_set: "macos-25B999-default".to_string(),
            appearance: ParityAppearance::Dark,
            scale: DisplayScale::Two,
            color_profile: ColorProfile::DisplayP3,
            window_size: PixelSize::new(1440, 900),
            focus: ParityFocusState::Active,
            view_mode: ParityViewMode::Icon,
            fixture_root: PathBuf::from("fixtures/icon"),
        };

        let err = provenance.validate().unwrap_err();

        assert!(err
            .to_string()
            .contains("approved mask set `macos-25B999-default`"));
        assert!(err.to_string().contains("macOS build `25A354`"));
    }

    #[test]
    fn parity_capture_provenance_rejects_unresolved_system_appearance() {
        let provenance = ParityCaptureProvenance {
            macos_build: "25A354".to_string(),
            hardware_profile: "macbookpro18,3".to_string(),
            display_profile: "studio-display-p3".to_string(),
            app_version: "0.1.0".to_string(),
            fixture_manifest: "fixtures/manifest.tsv".to_string(),
            captured_at: "2026-08-27T00:00:00Z".to_string(),
            expires_at: None,
            capture_command: "screencapture:-x".to_string(),
            reviewer: "codex".to_string(),
            signer: "codex".to_string(),
            approved_mask_set: "macos-25A354-default".to_string(),
            appearance: ParityAppearance::System,
            scale: DisplayScale::Two,
            color_profile: ColorProfile::DisplayP3,
            window_size: PixelSize::new(1440, 900),
            focus: ParityFocusState::Active,
            view_mode: ParityViewMode::Icon,
            fixture_root: PathBuf::from("fixtures/icon"),
        };

        let err = provenance.validate().unwrap_err();

        assert!(err.to_string().contains("resolved light or dark"));
        assert!(err.to_string().contains("not system"));
    }

    #[test]
    fn versioned_parity_manifest_accepts_leap_day_capture_timestamp() {
        let root = unique_temp_dir("gfm-parity-gate-valid-captured-at");
        let inputs = parse_parity_gate_manifest(
            "manifest-version\t1\nprofile\tmacos-build=25A354\thardware-profile=macbookpro18,3\tdisplay-profile=studio-display-p3\tapp-version=0.1.0\tfixture-manifest=fixtures/manifest.tsv\tcaptured-at=2028-02-29T23:59:59Z\tcapture-command=screencapture:-x\treviewer=codex\tsigner=codex\tapproved-mask-set=macos-25A354-default\tappearance=dark\tscale=2x\tcolor-profile=display-p3\nentry\ttoolbar\tfinder.png\tgfm.png\t1\t1\t\t1440\t900\tactive\ticon\tfixtures/icon\n",
            &root,
        )
        .unwrap();

        assert_eq!(
            inputs[0].provenance.as_ref().unwrap().captured_at,
            "2028-02-29T23:59:59Z"
        );

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
        write_capture_provenance_artifacts_with_profile(
            &root,
            "fixtures/text",
            ParityAppearance::Dark,
            ColorProfile::DisplayP3,
        );
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
        assert!(bundle.provenance_path.exists());
        assert!(bundle.visual_diff_dir.join("000-text-diff.png").exists());
        assert!(bundle
            .source_artifact_dir
            .join("000-text-finder.rgba")
            .exists());
        assert!(bundle
            .source_artifact_dir
            .join("000-text-finder.provenance.tsv")
            .exists());
        assert!(bundle
            .source_artifact_dir
            .join("000-text-gfm.provenance.tsv")
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
        let provenance = fs::read_to_string(&bundle.provenance_path).unwrap();
        assert!(provenance.contains("surface\tmacos-build\thardware-profile"));
        assert!(provenance.contains("text\t25A354\tmacbookpro18,3"));
        assert!(provenance.contains("fixtures/text"), "{provenance}");
        let bundle_manifest = fs::read_to_string(&bundle.bundle_manifest_path).unwrap();
        assert!(
            bundle_manifest.starts_with("kind\tpath\tbytes\tfnv1a64\n"),
            "{bundle_manifest}"
        );
        assert!(
            bundle_manifest
                .lines()
                .any(|line| line.starts_with("finder-source\t")
                    && line.contains("000-text-finder.rgba\t8\t")
                    && line.split('\t').nth(3).is_some_and(|hash| hash.len() == 16)),
            "{bundle_manifest}"
        );
        assert!(
            bundle_manifest
                .lines()
                .any(|line| line.starts_with("finder-provenance\t")
                    && line.contains("000-text-finder.provenance.tsv\t")
                    && line.split('\t').nth(3).is_some_and(|hash| hash.len() == 16)),
            "{bundle_manifest}"
        );
        assert!(
            bundle_manifest
                .lines()
                .any(|line| line.starts_with("gfm-provenance\t")
                    && line.contains("000-text-gfm.provenance.tsv\t")
                    && line.split('\t').nth(3).is_some_and(|hash| hash.len() == 16)),
            "{bundle_manifest}"
        );
        assert!(
            bundle_manifest
                .lines()
                .any(|line| line.starts_with("visual-diffs\t") && line.ends_with("\t-\t-")),
            "{bundle_manifest}"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn review_bundle_tsv_escapes_control_character_paths_and_provenance() {
        let root = unique_temp_dir("gfm-parity-review-escaped-tsv");
        let expected = root.join("finder\tcapture.rgba");
        let actual = root.join("gfm\ncapture.rgba");
        let output = root.join("review\tbundle");
        fs::write(&expected, [0, 0, 0, 255]).unwrap();
        fs::write(&actual, [0, 0, 0, 255]).unwrap();
        fs::write(
            expected.with_extension("provenance.tsv"),
            "target\tfinder\nfixture-root\tfixtures\\troot\noutput\tfinder\\tcapture.rgba\n",
        )
        .unwrap();
        fs::write(
            actual.with_extension("provenance.tsv"),
            "target\tgfm\nfixture-root\tfixtures\\nroot\noutput\tgfm\\ncapture.rgba\n",
        )
        .unwrap();

        let report = ParityGateReport {
            manifest_path: Some(root.join("gate\rmanifest.tsv")),
            entries: vec![ParityGateEntryReport {
                input: ParityGateInput::new(
                    ParitySurface::Text,
                    &expected,
                    &actual,
                    PixelSize::new(1, 1),
                )
                .with_provenance(ParityCaptureProvenance {
                    macos_build: "25A354".to_string(),
                    hardware_profile: "macbookpro18,3".to_string(),
                    display_profile: "studio\tdisplay".to_string(),
                    app_version: "0.1.0".to_string(),
                    fixture_manifest: "fixtures\nmanifest.tsv".to_string(),
                    captured_at: "2026-08-27T00:00:00Z".to_string(),
                    expires_at: Some("2026-09-27T00:00:00Z".to_string()),
                    capture_command: "screencapture\t-x".to_string(),
                    reviewer: "reviewer|name\nline".to_string(),
                    signer: "signer|name\rline".to_string(),
                    approved_mask_set: "macos-25A354-default|reviewed".to_string(),
                    appearance: ParityAppearance::Dark,
                    scale: DisplayScale::Two,
                    color_profile: ColorProfile::DisplayP3,
                    window_size: PixelSize::new(1040, 720),
                    focus: ParityFocusState::Active,
                    view_mode: ParityViewMode::List,
                    fixture_root: root.join("fixtures|root\tline"),
                }),
                diff: empty_report(PixelSize::new(1, 1)),
                evaluation: PixelThresholdEvaluation {
                    threshold: PixelDriftThreshold::finder_strict(ParitySurface::Text),
                    passed: true,
                    violations: Vec::new(),
                },
            }],
        };

        let bundle = write_parity_review_bundle(report, &output).unwrap();
        let review_markdown = fs::read_to_string(&bundle.review_path).unwrap();
        let entries = fs::read_to_string(&bundle.entries_path).unwrap();
        let provenance = fs::read_to_string(&bundle.provenance_path).unwrap();
        let bundle_manifest = fs::read_to_string(&bundle.bundle_manifest_path).unwrap();

        assert!(entries.contains("finder capture.rgba"), "{entries}");
        assert!(entries.contains("gfm capture.rgba"), "{entries}");
        assert!(entries.contains("studio display"), "{entries}");
        assert!(entries.contains("fixtures manifest.tsv"), "{entries}");
        assert!(entries.contains("screencapture -x"), "{entries}");
        assert!(entries.contains("reviewer|name line"), "{entries}");
        assert!(entries.contains("signer|name line"), "{entries}");
        assert!(entries.contains("fixtures|root line"), "{entries}");
        assert!(!entries.contains("finder\tcapture.rgba"), "{entries}");
        assert!(!entries.contains("gfm\ncapture.rgba"), "{entries}");
        assert!(
            review_markdown.contains("fixtures\\|root line"),
            "{review_markdown}"
        );
        assert!(
            review_markdown.contains("reviewer\\|name line"),
            "{review_markdown}"
        );
        assert!(
            review_markdown.contains("signer\\|name line"),
            "{review_markdown}"
        );
        assert!(
            review_markdown.contains("macos-25A354-default\\|reviewed"),
            "{review_markdown}"
        );
        assert!(provenance.contains("studio display"), "{provenance}");
        assert!(provenance.contains("fixtures manifest.tsv"), "{provenance}");
        assert!(provenance.contains("fixtures|root line"), "{provenance}");
        assert!(!provenance.contains("studio\tdisplay"), "{provenance}");
        assert!(
            !provenance.contains("fixtures\nmanifest.tsv"),
            "{provenance}"
        );
        assert!(
            review_markdown.contains("gate manifest.tsv"),
            "{review_markdown}"
        );
        assert!(
            !review_markdown.contains("gate\rmanifest.tsv"),
            "{review_markdown}"
        );
        assert!(
            bundle_manifest.contains("review bundle/review.md"),
            "{bundle_manifest}"
        );
        assert!(
            bundle_manifest.contains("review bundle/review.md\t"),
            "{bundle_manifest}"
        );
        assert!(
            bundle_manifest.contains("review bundle/provenance.tsv"),
            "{bundle_manifest}"
        );
        assert!(
            bundle_manifest
                .lines()
                .filter(|line| !line.is_empty())
                .all(|line| line.split('\t').count() == 4),
            "{bundle_manifest}"
        );
        assert!(
            !bundle_manifest.contains("review\tbundle"),
            "{bundle_manifest}"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn review_bundle_rejects_claimed_provenance_without_source_artifact_provenance() {
        let root = unique_temp_dir("gfm-parity-review-missing-source-provenance");
        let expected = root.join("expected.rgba");
        let actual = root.join("actual.rgba");
        fs::write(&expected, [0, 0, 0, 255]).unwrap();
        fs::write(&actual, [0, 0, 0, 255]).unwrap();
        let report = ParityGateReport {
            manifest_path: Some(root.join("gate.tsv")),
            entries: vec![ParityGateEntryReport {
                input: ParityGateInput::new(
                    ParitySurface::Text,
                    &expected,
                    &actual,
                    PixelSize::new(1, 1),
                )
                .with_provenance(ParityCaptureProvenance {
                    macos_build: "25A354".to_string(),
                    hardware_profile: "macbookpro18,3".to_string(),
                    display_profile: "studio-display-p3".to_string(),
                    app_version: "0.1.0".to_string(),
                    fixture_manifest: "fixtures/manifest.tsv".to_string(),
                    captured_at: "2026-08-27T00:00:00Z".to_string(),
                    expires_at: None,
                    capture_command: "screencapture:-x".to_string(),
                    reviewer: "codex".to_string(),
                    signer: "codex".to_string(),
                    approved_mask_set: "macos-25A354-default".to_string(),
                    appearance: ParityAppearance::Dark,
                    scale: DisplayScale::Two,
                    color_profile: ColorProfile::DisplayP3,
                    window_size: PixelSize::new(1040, 720),
                    focus: ParityFocusState::Active,
                    view_mode: ParityViewMode::List,
                    fixture_root: root.join("fixtures/text"),
                }),
                diff: empty_report(PixelSize::new(1, 1)),
                evaluation: PixelThresholdEvaluation {
                    threshold: PixelDriftThreshold::finder_strict(ParitySurface::Text),
                    passed: true,
                    violations: Vec::new(),
                },
            }],
        };

        let err = write_parity_review_bundle(report, root.join("review")).unwrap_err();

        assert!(err
            .to_string()
            .contains("requires expected Finder capture provenance file"));
        assert!(err.to_string().contains("expected.provenance.tsv"));

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

    fn write_capture_provenance_artifacts(root: &Path, fixture_root: &str) {
        write_capture_provenance_artifacts_with_profile(
            root,
            fixture_root,
            ParityAppearance::Light,
            ColorProfile::SRgb,
        );
    }

    fn write_capture_provenance_artifacts_with_profile(
        root: &Path,
        fixture_root: &str,
        appearance: ParityAppearance,
        color_profile: ColorProfile,
    ) {
        fs::create_dir_all(root.join("fixtures")).unwrap();
        fs::create_dir_all(root.join(fixture_root)).unwrap();
        let scenario = Path::new(fixture_root)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap();
        let view = match scenario {
            "list" | "text" => "list",
            "column" | "sidebar" => "column",
            "gallery" | "search" => "gallery",
            _ => "icon",
        };
        fs::write(
            root.join("fixtures/manifest.tsv"),
            format!(
                "scenario\troot\tfinder-view\tfiles\tdirectories\n{}\t{}\t{}\t1\t0\n",
                scenario,
                root.join(fixture_root).display(),
                view
            ),
        )
        .unwrap();
        let profile = TestCaptureArtifactProfile {
            root,
            fixture_root,
            scenario,
            view,
            appearance,
            color_profile,
        };
        write_capture_artifact_provenance(&root.join("expected.rgba"), "finder", &profile);
        write_capture_artifact_provenance(&root.join("actual.rgba"), "gfm", &profile);
    }

    struct TestCaptureArtifactProfile<'a> {
        root: &'a Path,
        fixture_root: &'a str,
        scenario: &'a str,
        view: &'a str,
        appearance: ParityAppearance,
        color_profile: ColorProfile,
    }

    fn write_capture_artifact_provenance(
        output: &Path,
        target: &str,
        profile: &TestCaptureArtifactProfile<'_>,
    ) {
        fs::write(
            output.with_extension("provenance.tsv"),
            format!(
                "target\t{}\nfixture-root\t{}\noutput\t{}\nscenario\t{}\nview-mode\t{}\nmacos-build\t25A354\nhardware-profile\tmacbookpro18,3\ndisplay-profile\tstudio-display-p3\napp-version\t0.1.0\ncaptured-at\t2026-08-27T00:00:00Z\ncapture-command\tscreencapture:-x:-R:40,70,1040,720\nreviewer\tcodex\nsigner\tcodex\napproved-mask-set\tmacos-25A354-default\nappearance\t{}\nscale\t2x\ncolor-profile\t{}\nfocus\tactive\nwindow-region\t40,70,1040,720\n",
                target,
                profile.root.join(profile.fixture_root).display(),
                output.display(),
                profile.scenario,
                profile.view,
                profile.appearance.as_str(),
                profile.color_profile.as_str()
            ),
        )
        .unwrap();
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
