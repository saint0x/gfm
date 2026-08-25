use gfm_types::{GfmError, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const DEFAULT_BUNDLE_IDENTIFIER: &str = "com.saint0x.gfm";
const DEFAULT_EXECUTABLE: &str = "gfm";
const DEFAULT_MINIMUM_SYSTEM_VERSION: &str = "14.0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseArtifactSpec {
    pub app_path: PathBuf,
    pub expected_bundle_identifier: String,
    pub expected_executable: String,
    pub minimum_system_version: String,
    pub require_signed: bool,
    pub require_notarized: bool,
    pub assess_gatekeeper: bool,
}

impl ReleaseArtifactSpec {
    pub fn new(app_path: impl Into<PathBuf>) -> Self {
        Self {
            app_path: app_path.into(),
            expected_bundle_identifier: DEFAULT_BUNDLE_IDENTIFIER.to_string(),
            expected_executable: DEFAULT_EXECUTABLE.to_string(),
            minimum_system_version: DEFAULT_MINIMUM_SYSTEM_VERSION.to_string(),
            require_signed: true,
            require_notarized: true,
            assess_gatekeeper: true,
        }
    }

    pub fn unsigned_local(app_path: impl Into<PathBuf>) -> Self {
        Self {
            require_signed: false,
            require_notarized: false,
            assess_gatekeeper: false,
            ..Self::new(app_path)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseArtifactReport {
    pub app_path: PathBuf,
    pub bundle_identifier: String,
    pub executable_path: PathBuf,
    pub executable_bytes: u64,
    pub icon_path: PathBuf,
    pub minimum_system_version: String,
    pub has_folder_association: bool,
    pub has_file_association: bool,
    pub signature: SignatureStatus,
    pub notarization: GatekeeperAssessment,
    pub gatekeeper: GatekeeperAssessment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureStatus {
    Verified,
    NotRequired,
}

impl SignatureStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::NotRequired => "not-required",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatekeeperAssessment {
    Passed,
    NotRequired,
}

impl GatekeeperAssessment {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::NotRequired => "not-required",
        }
    }
}

pub fn validate_release_artifact(spec: &ReleaseArtifactSpec) -> Result<ReleaseArtifactReport> {
    validate_spec(spec)?;
    let contents = spec.app_path.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    ensure_dir(&contents)?;
    ensure_dir(&macos)?;
    ensure_dir(&resources)?;

    let info_plist = contents.join("Info.plist");
    ensure_file(&info_plist)?;
    let plist = fs::read_to_string(&info_plist).map_err(|err| GfmError::io(&info_plist, err))?;

    let bundle_identifier = plist_string(&plist, "CFBundleIdentifier")?;
    if bundle_identifier != spec.expected_bundle_identifier {
        return Err(GfmError::Format(format!(
            "bundle identifier `{bundle_identifier}` does not match expected `{}`",
            spec.expected_bundle_identifier
        )));
    }
    let executable = plist_string(&plist, "CFBundleExecutable")?;
    if executable != spec.expected_executable {
        return Err(GfmError::Format(format!(
            "bundle executable `{executable}` does not match expected `{}`",
            spec.expected_executable
        )));
    }
    let minimum_system_version = plist_string(&plist, "LSMinimumSystemVersion")?;
    require_minimum_version(
        &minimum_system_version,
        &spec.minimum_system_version,
        "LSMinimumSystemVersion",
    )?;

    let has_folder_association = plist.contains("<string>public.folder</string>");
    let has_file_association = plist.contains("<string>public.item</string>");
    if !has_folder_association || !has_file_association {
        return Err(GfmError::Format(
            "release artifact must declare Finder-compatible folder and file associations"
                .to_string(),
        ));
    }

    let executable_path = macos.join(&executable);
    ensure_executable(&executable_path)?;
    let executable_bytes = fs::metadata(&executable_path)
        .map_err(|err| GfmError::io(&executable_path, err))?
        .len();
    if executable_bytes == 0 {
        return Err(GfmError::Format(format!(
            "{} is empty",
            executable_path.display()
        )));
    }

    let icon_name = plist_string(&plist, "CFBundleIconFile")?;
    let icon_path = resources.join(icon_file_name(&icon_name));
    ensure_file(&icon_path)?;

    let signature = if spec.require_signed {
        verify_codesign(&spec.app_path)?;
        SignatureStatus::Verified
    } else {
        SignatureStatus::NotRequired
    };
    let notarization = if spec.require_notarized {
        validate_stapled_ticket(&spec.app_path)?;
        GatekeeperAssessment::Passed
    } else {
        GatekeeperAssessment::NotRequired
    };
    let gatekeeper = if spec.assess_gatekeeper {
        assess_gatekeeper(&spec.app_path)?;
        GatekeeperAssessment::Passed
    } else {
        GatekeeperAssessment::NotRequired
    };

    Ok(ReleaseArtifactReport {
        app_path: spec.app_path.clone(),
        bundle_identifier,
        executable_path,
        executable_bytes,
        icon_path,
        minimum_system_version,
        has_folder_association,
        has_file_association,
        signature,
        notarization,
        gatekeeper,
    })
}

fn validate_spec(spec: &ReleaseArtifactSpec) -> Result<()> {
    ensure_dir(&spec.app_path)?;
    if spec.app_path.extension().and_then(|ext| ext.to_str()) != Some("app") {
        return Err(GfmError::Format(format!(
            "{} must be a .app bundle",
            spec.app_path.display()
        )));
    }
    if spec.expected_bundle_identifier.trim().is_empty()
        || !spec.expected_bundle_identifier.contains('.')
    {
        return Err(GfmError::Format(
            "expected bundle identifier must be a reverse-DNS identifier".to_string(),
        ));
    }
    if spec.expected_executable.trim().is_empty() || spec.expected_executable.contains('/') {
        return Err(GfmError::Format(
            "expected executable must be a bundle-local file name".to_string(),
        ));
    }
    parse_version(&spec.minimum_system_version)?;
    Ok(())
}

fn plist_string(plist: &str, key: &str) -> Result<String> {
    let key_marker = format!("<key>{key}</key>");
    let key_start = plist
        .find(&key_marker)
        .ok_or_else(|| GfmError::Format(format!("Info.plist missing `{key}`")))?;
    let after_key = &plist[key_start + key_marker.len()..];
    let string_start = after_key
        .find("<string>")
        .ok_or_else(|| GfmError::Format(format!("Info.plist `{key}` is not a string")))?;
    let after_string = &after_key[string_start + "<string>".len()..];
    let string_end = after_string
        .find("</string>")
        .ok_or_else(|| GfmError::Format(format!("Info.plist `{key}` string is unterminated")))?;
    Ok(after_string[..string_end].to_string())
}

fn icon_file_name(icon: &str) -> String {
    if icon.ends_with(".icns") {
        icon.to_string()
    } else {
        format!("{icon}.icns")
    }
}

fn require_minimum_version(actual: &str, expected: &str, label: &str) -> Result<()> {
    if parse_version(actual)? < parse_version(expected)? {
        return Err(GfmError::Format(format!(
            "{label} `{actual}` is below required `{expected}`"
        )));
    }
    Ok(())
}

fn parse_version(value: &str) -> Result<Vec<u16>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(GfmError::Format("version cannot be empty".to_string()));
    }
    let mut parts: Vec<u16> = trimmed
        .split('.')
        .map(|part| {
            if part.is_empty() {
                return Err(GfmError::Format(format!("invalid version `{value}`")));
            }
            part.parse::<u16>()
                .map_err(|_| GfmError::Format(format!("invalid numeric version `{value}`")))
        })
        .collect::<Result<_>>()?;
    while parts.len() > 1 && parts.last() == Some(&0) {
        parts.pop();
    }
    Ok(parts)
}

#[cfg(unix)]
fn ensure_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    ensure_file(path)?;
    let mode = fs::metadata(path)
        .map_err(|err| GfmError::io(path, err))?
        .permissions()
        .mode();
    if mode & 0o111 == 0 {
        return Err(GfmError::Format(format!(
            "{} is not executable",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_executable(path: &Path) -> Result<()> {
    ensure_file(path)
}

fn verify_codesign(app_path: &Path) -> Result<()> {
    run_command(
        Command::new("codesign")
            .arg("--verify")
            .arg("--strict")
            .arg("--deep")
            .arg("--verbose=2")
            .arg(app_path),
        app_path,
        "verify release signature",
    )
}

fn validate_stapled_ticket(app_path: &Path) -> Result<()> {
    run_command(
        Command::new("xcrun")
            .arg("stapler")
            .arg("validate")
            .arg(app_path),
        app_path,
        "validate stapled notarization ticket",
    )
}

fn assess_gatekeeper(app_path: &Path) -> Result<()> {
    run_command(
        Command::new("spctl")
            .arg("--assess")
            .arg("--type")
            .arg("execute")
            .arg("--verbose=4")
            .arg(app_path),
        app_path,
        "assess release artifact with Gatekeeper",
    )
}

fn run_command(command: &mut Command, path: &Path, label: &str) -> Result<()> {
    let output = command.output().map_err(|err| GfmError::io(path, err))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(GfmError::Format(format!(
        "{label} failed for {} with status {}; {}",
        path.display(),
        output.status,
        stderr.trim()
    )))
}

fn ensure_dir(path: &Path) -> Result<()> {
    if path.is_dir() {
        Ok(())
    } else {
        Err(GfmError::Format(format!(
            "{} is missing or is not a directory",
            path.display()
        )))
    }
}

fn ensure_file(path: &Path) -> Result<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(GfmError::Format(format!(
            "{} is missing or is not a file",
            path.display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build_app_bundle, AppBundleSpec, SigningIdentity};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn validates_unsigned_local_release_artifact() {
        let root = temp_root("unsigned-artifact");
        let executable = std::env::current_exe().expect("current test executable");
        let icon = root.join("GFM.icns");
        fs::create_dir_all(&root).expect("create temp root");
        fs::write(&icon, b"icns-test").expect("write icon");
        let mut bundle = AppBundleSpec::new(executable, &icon, root.join("dist"));
        bundle.signing_identity = SigningIdentity::Unsigned;
        let app = build_app_bundle(&bundle).expect("build bundle");

        let report = validate_release_artifact(&ReleaseArtifactSpec::unsigned_local(&app.app_path))
            .expect("validate release artifact");

        assert_eq!(report.bundle_identifier, DEFAULT_BUNDLE_IDENTIFIER);
        assert_eq!(report.signature, SignatureStatus::NotRequired);
        assert_eq!(report.notarization, GatekeeperAssessment::NotRequired);
        assert!(report.has_folder_association);
        assert!(report.has_file_association);
        assert!(report.executable_bytes > 0);
    }

    #[test]
    fn rejects_release_artifact_without_file_associations() {
        let root = temp_root("missing-associations");
        let executable = std::env::current_exe().expect("current test executable");
        let icon = root.join("GFM.icns");
        fs::create_dir_all(&root).expect("create temp root");
        fs::write(&icon, b"icns-test").expect("write icon");
        let mut bundle = AppBundleSpec::new(executable, &icon, root.join("dist"));
        bundle.signing_identity = SigningIdentity::Unsigned;
        let app = build_app_bundle(&bundle).expect("build bundle");
        let info = app.app_path.join("Contents/Info.plist");
        let plist = fs::read_to_string(&info).expect("read plist");
        fs::write(&info, plist.replace("<string>public.item</string>", "")).expect("damage plist");

        let err = validate_release_artifact(&ReleaseArtifactSpec::unsigned_local(&app.app_path))
            .expect_err("missing association fails");

        assert!(err.to_string().contains("folder and file associations"));
    }

    #[test]
    fn production_validation_requires_signature_by_default() {
        let root = temp_root("requires-signature");
        let executable = std::env::current_exe().expect("current test executable");
        let icon = root.join("GFM.icns");
        fs::create_dir_all(&root).expect("create temp root");
        fs::write(&icon, b"icns-test").expect("write icon");
        let mut bundle = AppBundleSpec::new(executable, &icon, root.join("dist"));
        bundle.signing_identity = SigningIdentity::Unsigned;
        let app = build_app_bundle(&bundle).expect("build bundle");

        let err = validate_release_artifact(&ReleaseArtifactSpec::new(&app.app_path))
            .expect_err("unsigned production artifact fails");

        assert!(err.to_string().contains("verify release signature"));
    }

    fn temp_root(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        std::env::temp_dir().join(format!("gfm-artifact-{name}-{nonce}"))
    }
}
