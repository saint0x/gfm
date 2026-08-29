use gfm_types::{GfmError, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const RELEASE_XCRUN_UTILITIES: &[&str] = &["ditto", "notarytool", "stapler", "metal", "metallib"];
const CODESIGN_UTILITIES: &[&str] = &["codesign"];
const RELEASE_DEVELOPER_DIR_ENV: &str = "GFM_RELEASE_DEVELOPER_DIR";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppleToolchainReport {
    pub developer_dir: PathBuf,
    pub utilities: Vec<AppleToolchainUtility>,
    pub metal_smoke_tested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppleToolchainUtility {
    pub name: String,
    pub path: PathBuf,
}

pub fn require_codesign_toolchain() -> Result<AppleToolchainReport> {
    require_toolchain("codesign", CODESIGN_UTILITIES, &[])
}

pub fn require_release_xcode_toolchain() -> Result<AppleToolchainReport> {
    require_toolchain(
        "production release",
        CODESIGN_UTILITIES,
        RELEASE_XCRUN_UTILITIES,
    )
}

fn require_toolchain(
    label: &str,
    path_utilities: &[&str],
    xcrun_utilities: &[&str],
) -> Result<AppleToolchainReport> {
    let metal_smoke_required =
        xcrun_utilities.contains(&"metal") && xcrun_utilities.contains(&"metallib");
    let developer_dir = developer_dir_for_toolchain(label, metal_smoke_required)?;
    let mut utilities = Vec::with_capacity(path_utilities.len() + xcrun_utilities.len() + 1);
    utilities.push(AppleToolchainUtility {
        name: "xcrun".to_string(),
        path: require_path_utility("xcrun", label)?,
    });
    for utility in path_utilities {
        utilities.push(AppleToolchainUtility {
            name: (*utility).to_string(),
            path: require_path_utility(utility, label)?,
        });
    }
    for utility in xcrun_utilities {
        utilities.push(AppleToolchainUtility {
            name: (*utility).to_string(),
            path: require_xcrun_utility(utility, &developer_dir, label)?,
        });
    }
    if metal_smoke_required {
        let metal = resolved_utility_path(&utilities, "metal", label)?;
        let metallib = resolved_utility_path(&utilities, "metallib", label)?;
        validate_metal_toolchain(&developer_dir, metal, metallib, label)?;
    }
    Ok(AppleToolchainReport {
        developer_dir,
        utilities,
        metal_smoke_tested: metal_smoke_required,
    })
}

fn require_full_xcode_developer_dir(developer_dir: &Path, label: &str) -> Result<()> {
    if is_full_xcode_developer_dir(developer_dir) {
        return Ok(());
    }

    Err(GfmError::Format(format!(
        "{label} requires Apple's full Xcode Metal toolchain; selected developer directory is {}. Command Line Tools do not ship the production `metal` and `metallib` tools required for release validation. Install full Xcode and select it with `sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer`, or set `{RELEASE_DEVELOPER_DIR_ENV}` to a full Xcode Developer directory for this release command",
        developer_dir.display()
    )))
}

fn developer_dir_for_toolchain(label: &str, require_metal: bool) -> Result<PathBuf> {
    if let Some(developer_dir) = explicit_release_developer_dir()? {
        if require_metal {
            require_full_xcode_developer_dir(&developer_dir, label)?;
        }
        return Ok(developer_dir);
    }

    let selected = selected_developer_dir()?;
    if !require_metal || is_full_xcode_developer_dir(&selected) {
        return Ok(selected);
    }

    if let Some(discovered) = discover_full_xcode_developer_dir() {
        return Ok(discovered);
    }

    require_full_xcode_developer_dir(&selected, label)?;
    Ok(selected)
}

fn explicit_release_developer_dir() -> Result<Option<PathBuf>> {
    let Some(value) = std::env::var_os(RELEASE_DEVELOPER_DIR_ENV) else {
        return Ok(None);
    };
    explicit_release_developer_dir_from_value(value)
}

fn explicit_release_developer_dir_from_value(
    value: impl Into<std::ffi::OsString>,
) -> Result<Option<PathBuf>> {
    let value = value.into();
    if value.is_empty() {
        return Err(GfmError::Format(format!(
            "`{RELEASE_DEVELOPER_DIR_ENV}` is set but empty"
        )));
    }
    let path = PathBuf::from(value);
    match fs::metadata(&path) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(explicit_release_developer_dir_not_directory_message(&path)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(explicit_release_developer_dir_not_directory_message(&path));
        }
        Err(err) => {
            return Err(GfmError::io(
                &path,
                format!("{RELEASE_DEVELOPER_DIR_ENV} metadata unavailable: {err}"),
            ));
        }
    }
    Ok(Some(path))
}

fn explicit_release_developer_dir_not_directory_message(path: &Path) -> GfmError {
    GfmError::Format(format!(
        "`{RELEASE_DEVELOPER_DIR_ENV}` points to {}, which is not a directory",
        path.display()
    ))
}

fn is_full_xcode_developer_dir(developer_dir: &Path) -> bool {
    let Some(contents_dir) = developer_dir.parent() else {
        return false;
    };
    if developer_dir.file_name().and_then(|name| name.to_str()) != Some("Developer") {
        return false;
    }
    if contents_dir.file_name().and_then(|name| name.to_str()) != Some("Contents") {
        return false;
    }
    contents_dir
        .parent()
        .and_then(|bundle| bundle.extension())
        .and_then(|extension| extension.to_str())
        == Some("app")
}

fn discover_full_xcode_developer_dir() -> Option<PathBuf> {
    candidate_xcode_developer_dirs()
        .into_iter()
        .find(|developer_dir| has_metal_xcrun_tools(developer_dir))
}

fn candidate_xcode_developer_dirs() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for app in [
        PathBuf::from("/Applications/Xcode.app"),
        PathBuf::from("/Applications/Xcode-beta.app"),
        PathBuf::from("/Applications/Xcode-Beta.app"),
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("Applications/Xcode.app"),
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("Applications/Xcode-beta.app"),
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("Applications/Xcode-Beta.app"),
    ] {
        let developer_dir = app.join("Contents/Developer");
        if is_full_xcode_developer_dir(&developer_dir) && developer_dir.is_dir() {
            candidates.push(developer_dir);
        }
    }
    candidates
}

fn has_metal_xcrun_tools(developer_dir: &Path) -> bool {
    ["metal", "metallib"]
        .iter()
        .all(|tool| xcrun_find(tool, developer_dir, "production release").is_ok())
}

fn selected_developer_dir() -> Result<PathBuf> {
    let output = Command::new("xcode-select")
        .arg("-p")
        .output()
        .map_err(|err| GfmError::Format(format!("failed to inspect selected Xcode path: {err}")))?;
    if !output.status.success() {
        return Err(command_failure(
            "selected Xcode path",
            "xcode-select -p",
            output,
            None,
        ));
    }
    let stdout = String::from_utf8(output.stdout).map_err(|err| {
        GfmError::Format(format!(
            "xcode-select returned non-UTF-8 developer path: {err}"
        ))
    })?;
    let path = stdout.trim();
    if path.is_empty() {
        return Err(GfmError::Format(
            "xcode-select returned an empty developer path".to_string(),
        ));
    }
    Ok(PathBuf::from(path))
}

fn require_path_utility(name: &str, label: &str) -> Result<PathBuf> {
    let output = Command::new("command")
        .arg("-v")
        .arg(name)
        .output()
        .or_else(|_| {
            Command::new("/bin/sh")
                .arg("-c")
                .arg(format!("command -v {}", shell_quote(name)))
                .output()
        })
        .map_err(|err| {
            GfmError::Format(format!("failed to inspect `{name}` for {label}: {err}"))
        })?;
    if !output.status.success() {
        return Err(command_failure(
            label,
            &format!("command -v {name}"),
            output,
            None,
        ));
    }
    path_from_stdout(name, label, output.stdout)
}

fn require_xcrun_utility(name: &str, developer_dir: &Path, label: &str) -> Result<PathBuf> {
    let output = xcrun_find(name, developer_dir, label)?;
    path_from_stdout(name, label, output.stdout)
}

fn xcrun_find(name: &str, developer_dir: &Path, label: &str) -> Result<std::process::Output> {
    let output = xcrun(developer_dir)
        .arg("--find")
        .arg(name)
        .output()
        .map_err(|err| GfmError::Format(format!("failed to inspect Apple `{name}` tool: {err}")))?;
    if !output.status.success() {
        return Err(command_failure(
            label,
            &format!(
                "DEVELOPER_DIR={} xcrun --find {name}",
                developer_dir.display()
            ),
            output,
            Some(developer_dir),
        ));
    }
    Ok(output)
}

fn resolved_utility_path<'a>(
    utilities: &'a [AppleToolchainUtility],
    name: &str,
    label: &str,
) -> Result<&'a Path> {
    utilities
        .iter()
        .find(|utility| utility.name == name)
        .map(|utility| utility.path.as_path())
        .ok_or_else(|| {
            GfmError::Format(format!(
                "{label} Apple toolchain resolved no `{name}` executable"
            ))
        })
}

fn validate_metal_toolchain(
    developer_dir: &Path,
    metal_path: &Path,
    metallib_path: &Path,
    label: &str,
) -> Result<()> {
    let root = unique_temp_dir("gfm-metal-toolchain")?;
    let source = root.join("gfm_toolchain_probe.metal");
    let air = root.join("gfm_toolchain_probe.air");
    let metallib = root.join("gfm_toolchain_probe.metallib");

    let result = (|| {
        fs::write(&source, metal_probe_source()).map_err(|err| GfmError::io(&source, err))?;

        let metal = apple_tool(developer_dir, metal_path)
            .arg("-c")
            .arg(&source)
            .arg("-o")
            .arg(&air)
            .output()
            .map_err(|err| {
                GfmError::Format(format!("failed to execute Apple Metal compiler: {err}"))
            })?;
        if !metal.status.success() {
            return Err(command_failure(
                label,
                &format!(
                    "DEVELOPER_DIR={} {} -c <probe.metal> -o <probe.air>",
                    developer_dir.display(),
                    metal_path.display()
                ),
                metal,
                Some(developer_dir),
            ));
        }

        let link = apple_tool(developer_dir, metallib_path)
            .arg(&air)
            .arg("-o")
            .arg(&metallib)
            .output()
            .map_err(|err| {
                GfmError::Format(format!(
                    "failed to execute Apple Metal library linker: {err}"
                ))
            })?;
        if !link.status.success() {
            return Err(command_failure(
                label,
                &format!(
                    "DEVELOPER_DIR={} {} <probe.air> -o <probe.metallib>",
                    developer_dir.display(),
                    metallib_path.display()
                ),
                link,
                Some(developer_dir),
            ));
        }

        let metadata = fs::metadata(&metallib).map_err(|err| GfmError::io(&metallib, err))?;
        if metadata.len() == 0 {
            return Err(GfmError::Format(format!(
                "{label} Apple Metal smoke test produced an empty metallib at {}",
                metallib.display()
            )));
        }
        Ok(())
    })();

    let cleanup = fs::remove_dir_all(&root);
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(err)) => Err(GfmError::io(&root, err)),
        (Err(err), _) => Err(err),
    }
}

fn unique_temp_dir(prefix: &str) -> Result<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| {
            GfmError::Format(format!(
                "failed to allocate Metal toolchain temp path: {err}"
            ))
        })?
        .as_nanos();
    let path = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&path).map_err(|err| GfmError::io(&path, err))?;
    Ok(path)
}

fn metal_probe_source() -> &'static str {
    r#"#include <metal_stdlib>
using namespace metal;

kernel void gfm_toolchain_probe(device uint *values [[buffer(0)]],
                                uint index [[thread_position_in_grid]]) {
    values[index] = values[index] + 1u;
}
"#
}

fn path_from_stdout(name: &str, label: &str, stdout: Vec<u8>) -> Result<PathBuf> {
    let stdout = String::from_utf8(stdout).map_err(|err| {
        GfmError::Format(format!("{label} `{name}` lookup returned non-UTF-8: {err}"))
    })?;
    let path = stdout.trim();
    if path.is_empty() {
        return Err(GfmError::Format(format!(
            "{label} toolchain lookup for `{name}` returned an empty path"
        )));
    }
    Ok(PathBuf::from(path))
}

fn command_failure(
    label: &str,
    command: &str,
    output: std::process::Output,
    developer_dir: Option<&Path>,
) -> GfmError {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let developer_dir = developer_dir
        .map(|path| format!(" selected developer directory: {}.", path.display()))
        .unwrap_or_default();
    GfmError::Format(format!(
        "{label} requires Apple's full release toolchain; `{command}` failed with status {};{} stdout: {}; stderr: {}; install full Xcode and select it with `sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer`",
        output.status,
        developer_dir,
        stdout.trim(),
        stderr.trim()
    ))
}

fn xcrun(developer_dir: &Path) -> Command {
    let mut command = Command::new("xcrun");
    command.env("DEVELOPER_DIR", developer_dir);
    command
}

fn apple_tool(developer_dir: &Path, path: &Path) -> Command {
    let mut command = Command::new(path);
    command.env("DEVELOPER_DIR", developer_dir);
    command
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_missing_xcrun_tool_with_release_guidance() {
        let developer_dir = PathBuf::from("/Library/Developer/CommandLineTools");
        let err = require_xcrun_utility(
            "gfm-definitely-missing-apple-tool",
            &developer_dir,
            "production release",
        )
        .expect_err("missing xcrun tool fails");
        let message = err.to_string();

        assert!(message.contains("production release requires Apple's full release toolchain"));
        assert!(message.contains("DEVELOPER_DIR=/Library/Developer/CommandLineTools"));
        assert!(message.contains("xcrun --find gfm-definitely-missing-apple-tool"));
        assert!(message.contains("xcode-select --switch"));
    }

    #[test]
    fn rejects_command_line_tools_for_release_metal_validation() {
        let developer_dir = PathBuf::from("/Library/Developer/CommandLineTools");
        let err = require_full_xcode_developer_dir(&developer_dir, "production release")
            .expect_err("CLT-only developer dir cannot release Metal builds");
        let message = err.to_string();

        assert!(message.contains("production release requires Apple's full Xcode Metal toolchain"));
        assert!(message.contains("/Library/Developer/CommandLineTools"));
        assert!(message.contains("metal"));
        assert!(message.contains("metallib"));
        assert!(message.contains(RELEASE_DEVELOPER_DIR_ENV));
        assert!(message.contains("xcode-select --switch"));
    }

    #[test]
    fn accepts_full_xcode_developer_directories() {
        assert!(is_full_xcode_developer_dir(&PathBuf::from(
            "/Applications/Xcode.app/Contents/Developer"
        )));
        assert!(is_full_xcode_developer_dir(&PathBuf::from(
            "/Volumes/Tools/Xcode-Beta.app/Contents/Developer"
        )));
        assert!(!is_full_xcode_developer_dir(&PathBuf::from(
            "/Library/Developer/CommandLineTools"
        )));
    }

    #[test]
    fn explicit_release_developer_dir_surfaces_metadata_probe_errors() {
        let path = std::env::temp_dir().join("gfm-release-developer-dir-unavailable".repeat(16));

        let err = explicit_release_developer_dir_from_value(path.as_os_str())
            .expect_err("unprobeable explicit developer dir fails");

        assert!(err
            .to_string()
            .contains("GFM_RELEASE_DEVELOPER_DIR metadata unavailable"));
        assert!(err
            .to_string()
            .contains("gfm-release-developer-dir-unavailable"));
    }

    #[test]
    fn validates_shell_quote_for_path_lookup_fallback() {
        assert_eq!(shell_quote("codesign"), "'codesign'");
        assert_eq!(shell_quote("weird'tool"), "'weird'\\''tool'");
    }

    #[test]
    fn metal_probe_source_is_a_real_kernel() {
        let source = metal_probe_source();

        assert!(source.contains("#include <metal_stdlib>"));
        assert!(source.contains("kernel void gfm_toolchain_probe"));
        assert!(source.contains("thread_position_in_grid"));
    }

    #[test]
    fn resolves_smoke_test_tools_from_reported_utilities() {
        let utilities = vec![
            AppleToolchainUtility {
                name: "metal".to_string(),
                path: PathBuf::from("/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/metal"),
            },
            AppleToolchainUtility {
                name: "metallib".to_string(),
                path: PathBuf::from("/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/metallib"),
            },
        ];

        assert_eq!(
            resolved_utility_path(&utilities, "metal", "production release")
                .expect("metal utility path"),
            Path::new(
                "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/metal"
            )
        );
        assert_eq!(
            resolved_utility_path(&utilities, "metallib", "production release")
                .expect("metallib utility path"),
            Path::new(
                "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/metallib"
            )
        );
    }

    #[test]
    fn rejects_missing_resolved_smoke_test_tool() {
        let err = resolved_utility_path(&[], "metal", "production release")
            .expect_err("missing Metal utility must fail");

        assert!(err
            .to_string()
            .contains("production release Apple toolchain resolved no `metal` executable"));
    }
}
