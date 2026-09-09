use crate::{
    ColorProfile, DisplayScale, ParityAppearance, ParityFocusState, ParitySurface, ParityViewMode,
    PixelSize,
};
use gfm_types::{GfmError, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    let content = format!(
        "manifest-version\t1\nprofile\tmacos-build={}\thardware-profile={}\tdisplay-profile={}\tapp-version={}\tfixture-manifest={}\tcaptured-at={}\tcapture-command={}\treviewer={}\tsigner={}\tapproved-mask-set={}\tappearance={}\tscale={}\tcolor-profile={}\nentry\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        escape_tsv_field(&finder.macos_build),
        escape_tsv_field(&finder.hardware_profile),
        escape_tsv_field(&finder.display_profile),
        escape_tsv_field(&finder.app_version),
        escape_tsv_field(&finder.fixture_root.join("manifest.tsv").to_string_lossy()),
        escape_tsv_field(&finder.captured_at),
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

fn validate_capture_options(options: &ParityScreenshotCaptureOptions) -> Result<()> {
    validate_capture_metadata(options)?;
    if options.target == ParityCaptureTarget::Gfm && options.gfm_app.is_none() {
        return Err(GfmError::Format(
            "gfm parity capture requires a GFM app path".to_string(),
        ));
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
                "/usr/bin/open".to_string(),
                "-a".to_string(),
                app.display().to_string(),
                "--args".to_string(),
                options.fixture_root.display().to_string(),
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
        .map_err(|err| GfmError::io(program, err))?;
    if !output.status.success() {
        return Err(GfmError::Format(format!(
            "{label} failed with status {}; stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

fn write_capture_provenance(
    options: &ParityScreenshotCaptureOptions,
    region: &CaptureRegion,
) -> Result<()> {
    let content = format!(
        "target\t{}\nfixture-root\t{}\noutput\t{}\nscenario\t{}\nview-mode\t{}\nmacos-build\t{}\nhardware-profile\t{}\ndisplay-profile\t{}\napp-version\t{}\ncaptured-at\t{}\ncapture-command\t{}\nreviewer\t{}\nsigner\t{}\napproved-mask-set\t{}\nappearance\t{}\nscale\t{}\ncolor-profile\t{}\nfocus\t{}\nwindow-region\t{}\n",
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

fn escape_tsv_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
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

        assert_eq!(
            prepare,
            vec![
                "/usr/bin/open",
                "-a",
                "/Applications/GFM.app",
                "--args",
                root.to_str().unwrap()
            ]
        );
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
