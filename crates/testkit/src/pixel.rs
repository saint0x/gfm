use gfm_types::{GfmError, Result};
use std::fs;
use std::path::Path;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelSize {
    pub width: u32,
    pub height: u32,
}

impl PixelSize {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub fn pixel_count(self) -> Result<usize> {
        let pixels = u64::from(self.width) * u64::from(self.height);
        usize::try_from(pixels)
            .map_err(|_| GfmError::Format("pixel image is too large".to_string()))
    }

    pub fn rgba_len(self) -> Result<usize> {
        self.pixel_count()?
            .checked_mul(4)
            .ok_or_else(|| GfmError::Format("pixel image byte length overflowed".to_string()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelMaskRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl PixelMaskRect {
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn contains(self, x: u32, y: u32) -> bool {
        x >= self.x
            && y >= self.y
            && x < self.x.saturating_add(self.width)
            && y < self.y.saturating_add(self.height)
    }

    pub fn is_valid_for(self, size: PixelSize) -> bool {
        self.width > 0
            && self.height > 0
            && self.x < size.width
            && self.y < size.height
            && self.x.saturating_add(self.width) <= size.width
            && self.y.saturating_add(self.height) <= size.height
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelDiffOptions {
    pub size: PixelSize,
    pub masks: Vec<PixelMaskRect>,
    pub fail_on_masked_mismatch: bool,
}

impl PixelDiffOptions {
    pub fn strict(size: PixelSize) -> Self {
        Self {
            size,
            masks: Vec::new(),
            fail_on_masked_mismatch: false,
        }
    }

    pub fn with_masks(mut self, masks: Vec<PixelMaskRect>) -> Self {
        self.masks = masks;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelDiffReport {
    pub size: PixelSize,
    pub total_pixels: usize,
    pub mismatched_pixels: usize,
    pub unmasked_mismatches: usize,
    pub masked_mismatches: usize,
    pub masks: Vec<PixelMaskRect>,
    pub first_unmasked_mismatch: Option<PixelMismatch>,
}

impl PixelDiffReport {
    pub fn passed(&self) -> bool {
        self.unmasked_mismatches == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelMismatch {
    pub x: u32,
    pub y: u32,
    pub expected: [u8; 4],
    pub actual: [u8; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ParitySurface {
    Layout,
    Text,
    Icon,
    Selection,
    Focus,
    Hover,
    Toolbar,
    Thumbnail,
    Preview,
}

impl ParitySurface {
    pub const ALL: [Self; 9] = [
        Self::Layout,
        Self::Text,
        Self::Icon,
        Self::Selection,
        Self::Focus,
        Self::Hover,
        Self::Toolbar,
        Self::Thumbnail,
        Self::Preview,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Layout => "layout",
            Self::Text => "text",
            Self::Icon => "icon",
            Self::Selection => "selection",
            Self::Focus => "focus",
            Self::Hover => "hover",
            Self::Toolbar => "toolbar",
            Self::Thumbnail => "thumbnail",
            Self::Preview => "preview",
        }
    }
}

impl FromStr for ParitySurface {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "layout" => Ok(Self::Layout),
            "text" => Ok(Self::Text),
            "icon" => Ok(Self::Icon),
            "selection" => Ok(Self::Selection),
            "focus" => Ok(Self::Focus),
            "hover" => Ok(Self::Hover),
            "toolbar" => Ok(Self::Toolbar),
            "thumbnail" => Ok(Self::Thumbnail),
            "preview" => Ok(Self::Preview),
            _ => Err(format!("unknown parity surface: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelDriftThreshold {
    pub surface: ParitySurface,
    pub max_unmasked_mismatches: usize,
    pub max_masked_mismatches: Option<usize>,
    pub require_explicit_masks: bool,
}

impl PixelDriftThreshold {
    pub const fn finder_strict(surface: ParitySurface) -> Self {
        Self {
            surface,
            max_unmasked_mismatches: 0,
            max_masked_mismatches: None,
            require_explicit_masks: true,
        }
    }

    pub const fn as_tsv(self) -> ThresholdTsv {
        ThresholdTsv(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThresholdTsv(PixelDriftThreshold);

impl std::fmt::Display for ThresholdTsv {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let threshold = self.0;
        let max_masked = threshold
            .max_masked_mismatches
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unbounded-explicit".to_string());
        write!(
            formatter,
            "threshold\t{}\tunmasked<={}\tmasked<={}\texplicit-masks={}",
            threshold.surface.as_str(),
            threshold.max_unmasked_mismatches,
            max_masked,
            threshold.require_explicit_masks
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelThresholdEvaluation {
    pub threshold: PixelDriftThreshold,
    pub passed: bool,
    pub violations: Vec<PixelThresholdViolation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PixelThresholdViolation {
    UnmaskedMismatchBudgetExceeded { actual: usize, max: usize },
    MaskedMismatchBudgetExceeded { actual: usize, max: usize },
    MaskedMismatchWithoutExplicitMask { actual: usize },
}

impl PixelThresholdViolation {
    pub fn as_tsv(&self) -> String {
        match self {
            Self::UnmaskedMismatchBudgetExceeded { actual, max } => {
                format!("violation\tunmasked-mismatch-budget\tactual={actual}\tmax={max}")
            }
            Self::MaskedMismatchBudgetExceeded { actual, max } => {
                format!("violation\tmasked-mismatch-budget\tactual={actual}\tmax={max}")
            }
            Self::MaskedMismatchWithoutExplicitMask { actual } => {
                format!("violation\tmasked-without-explicit-mask\tactual={actual}")
            }
        }
    }
}

pub fn evaluate_pixel_threshold(
    report: &PixelDiffReport,
    threshold: PixelDriftThreshold,
) -> PixelThresholdEvaluation {
    let mut violations = Vec::new();
    if report.unmasked_mismatches > threshold.max_unmasked_mismatches {
        violations.push(PixelThresholdViolation::UnmaskedMismatchBudgetExceeded {
            actual: report.unmasked_mismatches,
            max: threshold.max_unmasked_mismatches,
        });
    }
    if let Some(max) = threshold.max_masked_mismatches {
        if report.masked_mismatches > max {
            violations.push(PixelThresholdViolation::MaskedMismatchBudgetExceeded {
                actual: report.masked_mismatches,
                max,
            });
        }
    }
    if threshold.require_explicit_masks && report.masked_mismatches > 0 && report.masks.is_empty() {
        violations.push(PixelThresholdViolation::MaskedMismatchWithoutExplicitMask {
            actual: report.masked_mismatches,
        });
    }

    PixelThresholdEvaluation {
        threshold,
        passed: violations.is_empty(),
        violations,
    }
}

pub fn diff_rgba(
    expected: &[u8],
    actual: &[u8],
    options: &PixelDiffOptions,
) -> Result<PixelDiffReport> {
    let expected_len = options.size.rgba_len()?;
    if expected.len() != expected_len {
        return Err(GfmError::Format(format!(
            "expected image has {} bytes; expected {expected_len}",
            expected.len()
        )));
    }
    if actual.len() != expected_len {
        return Err(GfmError::Format(format!(
            "actual image has {} bytes; expected {expected_len}",
            actual.len()
        )));
    }
    for mask in &options.masks {
        if !mask.is_valid_for(options.size) {
            return Err(GfmError::Format(format!(
                "mask {},{},{},{} is outside {}x{} image",
                mask.x, mask.y, mask.width, mask.height, options.size.width, options.size.height
            )));
        }
    }

    let mut mismatched_pixels = 0;
    let mut unmasked_mismatches = 0;
    let mut masked_mismatches = 0;
    let mut first_unmasked_mismatch = None;

    for y in 0..options.size.height {
        for x in 0..options.size.width {
            let offset =
                ((u64::from(y) * u64::from(options.size.width) + u64::from(x)) * 4) as usize;
            let expected_pixel = pixel_at(expected, offset);
            let actual_pixel = pixel_at(actual, offset);
            if expected_pixel == actual_pixel {
                continue;
            }

            mismatched_pixels += 1;
            let masked = options.masks.iter().any(|mask| mask.contains(x, y));
            if masked {
                masked_mismatches += 1;
            } else {
                unmasked_mismatches += 1;
                first_unmasked_mismatch.get_or_insert(PixelMismatch {
                    x,
                    y,
                    expected: expected_pixel,
                    actual: actual_pixel,
                });
            }
        }
    }

    if options.fail_on_masked_mismatch && masked_mismatches > 0 {
        unmasked_mismatches += masked_mismatches;
    }

    Ok(PixelDiffReport {
        size: options.size,
        total_pixels: options.size.pixel_count()?,
        mismatched_pixels,
        unmasked_mismatches,
        masked_mismatches,
        masks: options.masks.clone(),
        first_unmasked_mismatch,
    })
}

pub fn diff_rgba_files(
    expected_path: impl AsRef<Path>,
    actual_path: impl AsRef<Path>,
    options: &PixelDiffOptions,
) -> Result<PixelDiffReport> {
    let expected_path = expected_path.as_ref();
    let actual_path = actual_path.as_ref();
    let expected = fs::read(expected_path).map_err(|err| GfmError::io(expected_path, err))?;
    let actual = fs::read(actual_path).map_err(|err| GfmError::io(actual_path, err))?;
    diff_rgba(&expected, &actual, options)
}

pub fn read_mask_file(path: impl AsRef<Path>, size: PixelSize) -> Result<Vec<PixelMaskRect>> {
    let path = path.as_ref();
    let content = fs::read_to_string(path).map_err(|err| GfmError::io(path, err))?;
    parse_masks(&content, size)
}

pub fn parse_masks(content: &str, size: PixelSize) -> Result<Vec<PixelMaskRect>> {
    let mut masks = Vec::new();
    for (line_index, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 4 {
            return Err(GfmError::Format(format!(
                "mask line {} must contain x, y, width, height",
                line_index + 1
            )));
        }
        let mask = PixelMaskRect {
            x: parse_mask_field(fields[0], line_index, "x")?,
            y: parse_mask_field(fields[1], line_index, "y")?,
            width: parse_mask_field(fields[2], line_index, "width")?,
            height: parse_mask_field(fields[3], line_index, "height")?,
        };
        if !mask.is_valid_for(size) {
            return Err(GfmError::Format(format!(
                "mask line {} is outside {}x{} image",
                line_index + 1,
                size.width,
                size.height
            )));
        }
        masks.push(mask);
    }
    Ok(masks)
}

fn parse_mask_field(value: &str, line_index: usize, name: &str) -> Result<u32> {
    value.parse().map_err(|_| {
        GfmError::Format(format!(
            "mask line {} field {name} must be an unsigned integer",
            line_index + 1
        ))
    })
}

fn pixel_at(bytes: &[u8], offset: usize) -> [u8; 4] {
    [
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match_passes_without_masks() {
        let expected = vec![0, 0, 0, 255, 10, 10, 10, 255];
        let actual = expected.clone();
        let report = diff_rgba(
            &expected,
            &actual,
            &PixelDiffOptions::strict(PixelSize::new(2, 1)),
        )
        .unwrap();

        assert!(report.passed());
        assert_eq!(report.mismatched_pixels, 0);
    }

    #[test]
    fn unmasked_mismatch_fails_with_coordinates() {
        let expected = vec![0, 0, 0, 255, 10, 10, 10, 255];
        let actual = vec![0, 0, 0, 255, 9, 10, 10, 255];
        let report = diff_rgba(
            &expected,
            &actual,
            &PixelDiffOptions::strict(PixelSize::new(2, 1)),
        )
        .unwrap();

        assert!(!report.passed());
        assert_eq!(report.unmasked_mismatches, 1);
        assert_eq!(report.first_unmasked_mismatch.unwrap().x, 1);
    }

    #[test]
    fn explicit_mask_suppresses_only_masked_pixels() {
        let expected = vec![0, 0, 0, 255, 10, 10, 10, 255, 20, 20, 20, 255];
        let actual = vec![0, 0, 0, 255, 9, 10, 10, 255, 20, 21, 20, 255];
        let options = PixelDiffOptions::strict(PixelSize::new(3, 1))
            .with_masks(vec![PixelMaskRect::new(1, 0, 1, 1)]);
        let report = diff_rgba(&expected, &actual, &options).unwrap();

        assert!(!report.passed());
        assert_eq!(report.masked_mismatches, 1);
        assert_eq!(report.unmasked_mismatches, 1);
        assert_eq!(report.first_unmasked_mismatch.unwrap().x, 2);
    }

    #[test]
    fn parses_tab_separated_masks() {
        let masks = parse_masks("1\t2\t3\t4\n# comment\n", PixelSize::new(10, 10)).unwrap();

        assert_eq!(masks, vec![PixelMaskRect::new(1, 2, 3, 4)]);
    }

    #[test]
    fn strict_threshold_rejects_unmasked_text_drift() {
        let expected = vec![0, 0, 0, 255, 10, 10, 10, 255];
        let actual = vec![0, 0, 0, 255, 9, 10, 10, 255];
        let report = diff_rgba(
            &expected,
            &actual,
            &PixelDiffOptions::strict(PixelSize::new(2, 1)),
        )
        .unwrap();
        let evaluation = evaluate_pixel_threshold(
            &report,
            PixelDriftThreshold::finder_strict(ParitySurface::Text),
        );

        assert!(!evaluation.passed);
        assert_eq!(
            evaluation.violations,
            vec![PixelThresholdViolation::UnmaskedMismatchBudgetExceeded { actual: 1, max: 0 }]
        );
    }

    #[test]
    fn strict_threshold_allows_explicit_masked_drift() {
        let expected = vec![0, 0, 0, 255, 10, 10, 10, 255];
        let actual = vec![0, 0, 0, 255, 9, 10, 10, 255];
        let options = PixelDiffOptions::strict(PixelSize::new(2, 1))
            .with_masks(vec![PixelMaskRect::new(1, 0, 1, 1)]);
        let report = diff_rgba(&expected, &actual, &options).unwrap();
        let evaluation = evaluate_pixel_threshold(
            &report,
            PixelDriftThreshold::finder_strict(ParitySurface::Focus),
        );

        assert!(evaluation.passed);
        assert!(evaluation.violations.is_empty());
    }

    #[test]
    fn threshold_tsv_is_stable_for_cli_and_fozzy() {
        let threshold = PixelDriftThreshold::finder_strict(ParitySurface::Toolbar);

        assert_eq!(
            threshold.as_tsv().to_string(),
            "threshold\ttoolbar\tunmasked<=0\tmasked<=unbounded-explicit\texplicit-masks=true"
        );
    }
}
