use gfm_types::{GfmError, Result};
use std::path::PathBuf;
use std::process::Command;

const RELEASE_XCRUN_UTILITIES: &[&str] = &["ditto", "notarytool", "stapler", "metal", "metallib"];
const CODESIGN_UTILITIES: &[&str] = &["codesign"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppleToolchainReport {
    pub developer_dir: PathBuf,
    pub utilities: Vec<AppleToolchainUtility>,
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
    let developer_dir = selected_developer_dir()?;
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
    Ok(AppleToolchainReport {
        developer_dir,
        utilities,
    })
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

fn require_xcrun_utility(name: &str, developer_dir: &PathBuf, label: &str) -> Result<PathBuf> {
    let output = Command::new("xcrun")
        .arg("--find")
        .arg(name)
        .output()
        .map_err(|err| GfmError::Format(format!("failed to inspect Apple `{name}` tool: {err}")))?;
    if !output.status.success() {
        return Err(command_failure(
            label,
            &format!("xcrun --find {name}"),
            output,
            Some(developer_dir),
        ));
    }
    path_from_stdout(name, label, output.stdout)
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
    developer_dir: Option<&PathBuf>,
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
        assert!(message.contains("xcrun --find gfm-definitely-missing-apple-tool"));
        assert!(message.contains("xcode-select --switch"));
    }

    #[test]
    fn validates_shell_quote_for_path_lookup_fallback() {
        assert_eq!(shell_quote("codesign"), "'codesign'");
        assert_eq!(shell_quote("weird'tool"), "'weird'\\''tool'");
    }
}
