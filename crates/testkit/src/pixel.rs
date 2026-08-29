use gfm_types::{GfmError, Result};
use std::fs;
use std::io::BufWriter;
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
pub struct PixelMaskRegion {
    pub rect: PixelMaskRect,
    pub reason: String,
}

impl PixelMaskRegion {
    pub fn new(rect: PixelMaskRect, reason: impl Into<String>) -> Self {
        Self {
            rect,
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelDiffOptions {
    pub size: PixelSize,
    pub masks: Vec<PixelMaskRegion>,
    pub fail_on_masked_mismatch: bool,
    pub require_masked_drift: bool,
}

impl PixelDiffOptions {
    pub fn strict(size: PixelSize) -> Self {
        Self {
            size,
            masks: Vec::new(),
            fail_on_masked_mismatch: false,
            require_masked_drift: false,
        }
    }

    pub fn with_masks(mut self, masks: Vec<PixelMaskRect>) -> Self {
        self.masks = masks
            .into_iter()
            .map(|rect| PixelMaskRegion::new(rect, "legacy-explicit-mask"))
            .collect();
        self
    }

    pub fn with_governed_masks(mut self, masks: Vec<PixelMaskRegion>) -> Self {
        self.masks = masks;
        self.require_masked_drift = true;
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
    pub max_channel_delta: u8,
    pub masks: Vec<PixelMaskRegion>,
    pub regions: Vec<PixelRegionSummary>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelRegionSummary {
    pub name: String,
    pub rect: PixelMaskRect,
    pub mismatched_pixels: usize,
    pub max_channel_delta: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ParitySurface {
    Layout,
    Text,
    Icon,
    Sidebar,
    Selection,
    Focus,
    Hover,
    Toolbar,
    Thumbnail,
    Preview,
    Sheet,
    Menu,
}

impl ParitySurface {
    pub const ALL: [Self; 12] = [
        Self::Layout,
        Self::Text,
        Self::Icon,
        Self::Sidebar,
        Self::Selection,
        Self::Focus,
        Self::Hover,
        Self::Toolbar,
        Self::Thumbnail,
        Self::Preview,
        Self::Sheet,
        Self::Menu,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Layout => "layout",
            Self::Text => "text",
            Self::Icon => "icon",
            Self::Sidebar => "sidebar",
            Self::Selection => "selection",
            Self::Focus => "focus",
            Self::Hover => "hover",
            Self::Toolbar => "toolbar",
            Self::Thumbnail => "thumbnail",
            Self::Preview => "preview",
            Self::Sheet => "sheet",
            Self::Menu => "menu",
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
            "sidebar" => Ok(Self::Sidebar),
            "selection" => Ok(Self::Selection),
            "focus" => Ok(Self::Focus),
            "hover" => Ok(Self::Hover),
            "toolbar" => Ok(Self::Toolbar),
            "thumbnail" => Ok(Self::Thumbnail),
            "preview" => Ok(Self::Preview),
            "sheet" => Ok(Self::Sheet),
            "menu" => Ok(Self::Menu),
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
        if !mask.rect.is_valid_for(options.size) {
            return Err(GfmError::Format(format!(
                "mask {},{},{},{} is outside {}x{} image",
                mask.rect.x,
                mask.rect.y,
                mask.rect.width,
                mask.rect.height,
                options.size.width,
                options.size.height
            )));
        }
        if mask.reason.trim().is_empty() {
            return Err(GfmError::Format(
                "pixel mask reason cannot be empty".to_string(),
            ));
        }
    }

    let mut mismatched_pixels = 0;
    let mut unmasked_mismatches = 0;
    let mut masked_mismatches = 0;
    let mut max_channel_delta = 0;
    let mut regions = options
        .masks
        .iter()
        .map(|mask| PixelRegionSummary {
            name: mask.reason.clone(),
            rect: mask.rect,
            mismatched_pixels: 0,
            max_channel_delta: 0,
        })
        .collect::<Vec<_>>();
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
            let delta = channel_delta(expected_pixel, actual_pixel);
            max_channel_delta = max_channel_delta.max(delta);
            let mut masked = false;
            for (index, mask) in options.masks.iter().enumerate() {
                if mask.rect.contains(x, y) {
                    masked = true;
                    regions[index].mismatched_pixels += 1;
                    regions[index].max_channel_delta = regions[index].max_channel_delta.max(delta);
                }
            }
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
    if options.require_masked_drift {
        for region in &regions {
            if region.mismatched_pixels == 0 {
                return Err(GfmError::Format(format!(
                    "governed mask {},{},{},{} is loose or stale: no drift covered for `{}`",
                    region.rect.x,
                    region.rect.y,
                    region.rect.width,
                    region.rect.height,
                    region.name
                )));
            }
        }
    }

    Ok(PixelDiffReport {
        size: options.size,
        total_pixels: options.size.pixel_count()?,
        mismatched_pixels,
        unmasked_mismatches,
        masked_mismatches,
        max_channel_delta,
        masks: options.masks.clone(),
        regions,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub size: PixelSize,
    pub bytes: Vec<u8>,
}

pub fn read_rgba_image_file(
    path: impl AsRef<Path>,
    raw_size: Option<PixelSize>,
) -> Result<RgbaImage> {
    let path = path.as_ref();
    let bytes = fs::read(path).map_err(|err| GfmError::io(path, err))?;
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        decode_png_rgba(path, &bytes)
    } else {
        let size = raw_size.ok_or_else(|| {
            GfmError::Format(format!(
                "raw RGBA image {} requires an explicit width and height",
                path.display()
            ))
        })?;
        if bytes.len() != size.rgba_len()? {
            return Err(GfmError::Format(format!(
                "raw RGBA image {} has {} bytes; expected {}",
                path.display(),
                bytes.len(),
                size.rgba_len()?
            )));
        }
        Ok(RgbaImage { size, bytes })
    }
}

pub fn diff_image_files(
    expected_path: impl AsRef<Path>,
    actual_path: impl AsRef<Path>,
    raw_size: Option<PixelSize>,
    masks: Vec<PixelMaskRegion>,
) -> Result<(PixelDiffReport, RgbaImage, RgbaImage)> {
    let expected = read_rgba_image_file(expected_path, raw_size)?;
    let actual = read_rgba_image_file(actual_path, raw_size)?;
    if let Some(declared) = raw_size {
        if expected.size != declared {
            return Err(GfmError::Format(format!(
                "expected image dimensions {}x{} do not match declared {}x{}",
                expected.size.width, expected.size.height, declared.width, declared.height
            )));
        }
        if actual.size != declared {
            return Err(GfmError::Format(format!(
                "actual image dimensions {}x{} do not match declared {}x{}",
                actual.size.width, actual.size.height, declared.width, declared.height
            )));
        }
    }
    if expected.size != actual.size {
        return Err(GfmError::Format(format!(
            "image dimensions differ: expected {}x{} actual {}x{}",
            expected.size.width, expected.size.height, actual.size.width, actual.size.height
        )));
    }
    let options = PixelDiffOptions::strict(expected.size).with_governed_masks(masks);
    let report = diff_rgba(&expected.bytes, &actual.bytes, &options)?;
    Ok((report, expected, actual))
}

pub fn write_visual_diff_png(
    path: impl AsRef<Path>,
    expected: &RgbaImage,
    actual: &RgbaImage,
    report: &PixelDiffReport,
) -> Result<()> {
    if expected.size != actual.size {
        return Err(GfmError::Format(
            "cannot write visual diff for different image sizes".to_string(),
        ));
    }
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| GfmError::io(parent, err))?;
    }
    let mut output = vec![0; expected.size.rgba_len()?];
    for y in 0..expected.size.height {
        for x in 0..expected.size.width {
            let offset =
                ((u64::from(y) * u64::from(expected.size.width) + u64::from(x)) * 4) as usize;
            let expected_pixel = pixel_at(&expected.bytes, offset);
            let actual_pixel = pixel_at(&actual.bytes, offset);
            if expected_pixel == actual_pixel {
                output[offset] = actual_pixel[0] / 4;
                output[offset + 1] = actual_pixel[1] / 4;
                output[offset + 2] = actual_pixel[2] / 4;
                output[offset + 3] = 96;
                continue;
            }
            let masked = report.masks.iter().any(|mask| mask.rect.contains(x, y));
            if masked {
                output[offset] = 0;
                output[offset + 1] = 96;
                output[offset + 2] = 255;
            } else {
                output[offset] = 255;
                output[offset + 1] = 32;
                output[offset + 2] = 48;
            }
            output[offset + 3] = 255;
        }
    }
    encode_png_rgba(path, expected.size, &output)
}

pub fn read_mask_file(path: impl AsRef<Path>, size: PixelSize) -> Result<Vec<PixelMaskRect>> {
    let path = path.as_ref();
    let content = fs::read_to_string(path).map_err(|err| GfmError::io(path, err))?;
    parse_masks(&content, size)
}

pub fn read_governed_mask_file(
    path: impl AsRef<Path>,
    size: PixelSize,
) -> Result<Vec<PixelMaskRegion>> {
    let path = path.as_ref();
    let content = fs::read_to_string(path).map_err(|err| GfmError::io(path, err))?;
    parse_governed_masks(&content, size)
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

pub fn parse_governed_masks(content: &str, size: PixelSize) -> Result<Vec<PixelMaskRegion>> {
    let mut masks = Vec::new();
    for (line_index, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 5 {
            return Err(GfmError::Format(format!(
                "governed mask line {} must contain x, y, width, height, reason",
                line_index + 1
            )));
        }
        let reason = fields[4].trim();
        if reason.is_empty() {
            return Err(GfmError::Format(format!(
                "governed mask line {} must include a reason",
                line_index + 1
            )));
        }
        let rect = PixelMaskRect {
            x: parse_mask_field(fields[0], line_index, "x")?,
            y: parse_mask_field(fields[1], line_index, "y")?,
            width: parse_mask_field(fields[2], line_index, "width")?,
            height: parse_mask_field(fields[3], line_index, "height")?,
        };
        if !rect.is_valid_for(size) {
            return Err(GfmError::Format(format!(
                "governed mask line {} is outside {}x{} image",
                line_index + 1,
                size.width,
                size.height
            )));
        }
        masks.push(PixelMaskRegion::new(rect, reason));
    }
    Ok(masks)
}

fn decode_png_rgba(path: &Path, bytes: &[u8]) -> Result<RgbaImage> {
    let decoder = png::Decoder::new(bytes);
    let mut reader = decoder
        .read_info()
        .map_err(|err| GfmError::Format(format!("failed to read PNG {}: {err}", path.display())))?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer).map_err(|err| {
        GfmError::Format(format!("failed to decode PNG {}: {err}", path.display()))
    })?;
    let source = &buffer[..info.buffer_size()];
    let rgba = match (info.color_type, info.bit_depth) {
        (png::ColorType::Rgba, png::BitDepth::Eight) => source.to_vec(),
        (png::ColorType::Rgb, png::BitDepth::Eight) => {
            let mut out = Vec::with_capacity(source.len() / 3 * 4);
            for pixel in source.chunks_exact(3) {
                out.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            }
            out
        }
        (png::ColorType::Grayscale, png::BitDepth::Eight) => source
            .iter()
            .flat_map(|value| [*value, *value, *value, 255])
            .collect(),
        (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) => source
            .chunks_exact(2)
            .flat_map(|pixel| [pixel[0], pixel[0], pixel[0], pixel[1]])
            .collect(),
        _ => {
            return Err(GfmError::Format(format!(
                "unsupported PNG format for {}: {:?} {:?}",
                path.display(),
                info.color_type,
                info.bit_depth
            )))
        }
    };
    Ok(RgbaImage {
        size: PixelSize::new(info.width, info.height),
        bytes: rgba,
    })
}

fn encode_png_rgba(path: &Path, size: PixelSize, bytes: &[u8]) -> Result<()> {
    let file = fs::File::create(path).map_err(|err| GfmError::io(path, err))?;
    let writer = BufWriter::new(file);
    let mut encoder = png::Encoder::new(writer, size.width, size.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut png_writer = encoder.write_header().map_err(|err| {
        GfmError::Format(format!("failed to write PNG {}: {err}", path.display()))
    })?;
    png_writer
        .write_image_data(bytes)
        .map_err(|err| GfmError::Format(format!("failed to write PNG {}: {err}", path.display())))
}

fn channel_delta(expected: [u8; 4], actual: [u8; 4]) -> u8 {
    expected
        .into_iter()
        .zip(actual)
        .map(|(left, right)| left.abs_diff(right))
        .max()
        .unwrap_or(0)
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
        assert_eq!(report.max_channel_delta, 1);
        assert_eq!(report.regions[0].mismatched_pixels, 1);
        assert_eq!(report.first_unmasked_mismatch.unwrap().x, 2);
    }

    #[test]
    fn parses_tab_separated_masks() {
        let masks = parse_masks("1\t2\t3\t4\n# comment\n", PixelSize::new(10, 10)).unwrap();

        assert_eq!(masks, vec![PixelMaskRect::new(1, 2, 3, 4)]);
    }

    #[test]
    fn parses_governed_masks_with_reasons() {
        let masks = parse_governed_masks("1\t2\t3\t4\tclock glyph blink\n", PixelSize::new(10, 10))
            .unwrap();

        assert_eq!(masks[0].rect, PixelMaskRect::new(1, 2, 3, 4));
        assert_eq!(masks[0].reason, "clock glyph blink");
    }

    #[test]
    fn governed_masks_reject_loose_regions_without_drift() {
        let expected = vec![0, 0, 0, 255, 10, 10, 10, 255];
        let actual = vec![0, 0, 0, 255, 9, 10, 10, 255];
        let options = PixelDiffOptions::strict(PixelSize::new(2, 1)).with_governed_masks(vec![
            PixelMaskRegion::new(PixelMaskRect::new(0, 0, 1, 1), "stale clock mask"),
        ]);

        let err = diff_rgba(&expected, &actual, &options).unwrap_err();

        assert!(err.to_string().contains("loose or stale"));
        assert!(err.to_string().contains("stale clock mask"));
    }

    #[test]
    fn legacy_masks_may_cover_zero_drift_for_raw_pixel_diff() {
        let expected = vec![0, 0, 0, 255, 10, 10, 10, 255];
        let actual = vec![0, 0, 0, 255, 9, 10, 10, 255];
        let options = PixelDiffOptions::strict(PixelSize::new(2, 1))
            .with_masks(vec![PixelMaskRect::new(0, 0, 1, 1)]);

        let report = diff_rgba(&expected, &actual, &options).unwrap();

        assert_eq!(report.unmasked_mismatches, 1);
        assert_eq!(report.masked_mismatches, 0);
    }

    #[test]
    fn rejects_governed_masks_without_reason() {
        let err = parse_governed_masks("1\t2\t3\t4\n", PixelSize::new(10, 10)).unwrap_err();

        assert!(err
            .to_string()
            .contains("must contain x, y, width, height, reason"));
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

    #[test]
    fn strict_surface_parser_includes_finder_sidebar_sheet_and_menu_regions() {
        assert_eq!(
            "sidebar".parse::<ParitySurface>().unwrap(),
            ParitySurface::Sidebar
        );
        assert_eq!(
            "sheet".parse::<ParitySurface>().unwrap(),
            ParitySurface::Sheet
        );
        assert_eq!(
            "menu".parse::<ParitySurface>().unwrap(),
            ParitySurface::Menu
        );
        assert!(ParitySurface::ALL.contains(&ParitySurface::Sidebar));
        assert!(ParitySurface::ALL.contains(&ParitySurface::Sheet));
        assert!(ParitySurface::ALL.contains(&ParitySurface::Menu));
        assert_eq!(
            PixelDriftThreshold::finder_strict(ParitySurface::Menu)
                .as_tsv()
                .to_string(),
            "threshold\tmenu\tunmasked<=0\tmasked<=unbounded-explicit\texplicit-masks=true"
        );
    }

    #[test]
    fn decodes_and_writes_png_artifacts() {
        let root = std::env::temp_dir().join(format!(
            "gfm-png-pixel-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let expected = RgbaImage {
            size: PixelSize::new(1, 1),
            bytes: vec![0, 0, 0, 255],
        };
        let actual = RgbaImage {
            size: PixelSize::new(1, 1),
            bytes: vec![255, 0, 0, 255],
        };
        let diff = diff_rgba(
            &expected.bytes,
            &actual.bytes,
            &PixelDiffOptions::strict(expected.size),
        )
        .unwrap();
        let diff_path = root.join("diff.png");

        write_visual_diff_png(&diff_path, &expected, &actual, &diff).unwrap();
        let decoded = read_rgba_image_file(&diff_path, None).unwrap();

        assert_eq!(decoded.size, PixelSize::new(1, 1));
        assert_eq!(decoded.bytes, vec![255, 32, 48, 255]);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn png_diff_rejects_manifest_declared_size_drift() {
        let root = std::env::temp_dir().join(format!(
            "gfm-png-declared-size-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let image = RgbaImage {
            size: PixelSize::new(1, 1),
            bytes: vec![0, 0, 0, 255],
        };
        let expected = root.join("expected.png");
        let actual = root.join("actual.png");
        let report = diff_rgba(
            &image.bytes,
            &image.bytes,
            &PixelDiffOptions::strict(image.size),
        )
        .unwrap();
        write_visual_diff_png(&expected, &image, &image, &report).unwrap();
        write_visual_diff_png(&actual, &image, &image, &report).unwrap();

        let err = diff_image_files(&expected, &actual, Some(PixelSize::new(2, 1)), Vec::new())
            .unwrap_err();

        assert!(err.to_string().contains("do not match declared 2x1"));
        fs::remove_dir_all(root).unwrap();
    }
}
