use crate::{
    diff_rgba_files, evaluate_pixel_threshold, read_mask_file, ParitySurface, PixelDiffOptions,
    PixelDiffReport, PixelDriftThreshold, PixelSize, PixelThresholdEvaluation,
};
use gfm_types::{GfmError, Result};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParityReviewBundle {
    pub output_dir: PathBuf,
    pub review_path: PathBuf,
    pub entries_path: PathBuf,
    pub violations_path: PathBuf,
    pub first_mismatch_path: PathBuf,
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
    let bundle_manifest_path = output_dir.join("bundle.tsv");

    write_text(&review_path, &render_review_markdown(&report))?;
    write_text(&entries_path, &render_entries_tsv(&report))?;
    write_text(&violations_path, &render_violations_tsv(&report))?;
    write_text(&first_mismatch_path, &render_first_mismatches_tsv(&report))?;
    write_text(
        &bundle_manifest_path,
        &render_bundle_manifest(
            &review_path,
            &entries_path,
            &violations_path,
            &first_mismatch_path,
        ),
    )?;

    Ok(ParityReviewBundle {
        output_dir,
        review_path,
        entries_path,
        violations_path,
        first_mismatch_path,
        bundle_manifest_path,
        report,
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
    text.push_str("## Surface Summary\n\n");
    text.push_str("| Surface | Size | Mismatched | Unmasked | Masked | Passed |\n");
    text.push_str("| --- | ---: | ---: | ---: | ---: | --- |\n");
    for entry in &report.entries {
        text.push_str(&format!(
            "| {} | {}x{} | {} | {} | {} | {} |\n",
            entry.input.surface.as_str(),
            entry.diff.size.width,
            entry.diff.size.height,
            entry.diff.mismatched_pixels,
            entry.diff.unmasked_mismatches,
            entry.diff.masked_mismatches,
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
        "surface\twidth\theight\texpected\tactual\tmask\tmismatched\tunmasked\tmasked\tpassed\n"
            .to_string();
    for entry in &report.entries {
        text.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
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
            entry.diff.mismatched_pixels,
            entry.diff.unmasked_mismatches,
            entry.diff.masked_mismatches,
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

fn render_bundle_manifest(
    review_path: &Path,
    entries_path: &Path,
    violations_path: &Path,
    first_mismatch_path: &Path,
) -> String {
    format!(
        "kind\tpath\nreview\t{}\nentries\t{}\nviolations\t{}\nfirst-unmasked\t{}\n",
        review_path.display(),
        entries_path.display(),
        violations_path.display(),
        first_mismatch_path.display()
    )
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
            "text\texpected.rgba\tactual.rgba\t2\t1\n",
        )
        .unwrap();

        let bundle = write_parity_review_bundle_manifest(root.join("gate.tsv"), &output).unwrap();

        assert!(!bundle.report.passed());
        assert!(bundle.review_path.exists());
        assert!(bundle.entries_path.exists());
        assert!(bundle.violations_path.exists());
        assert!(bundle.first_mismatch_path.exists());
        assert!(fs::read_to_string(&bundle.review_path)
            .unwrap()
            .contains("Passed: false"));
        assert!(fs::read_to_string(&bundle.violations_path)
            .unwrap()
            .contains("unmasked-mismatch-budget"));
        assert!(fs::read_to_string(&bundle.first_mismatch_path)
            .unwrap()
            .contains("090a0aff"));

        fs::remove_dir_all(root).unwrap();
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
