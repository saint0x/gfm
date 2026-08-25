use gfm_types::{GfmError, Result};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotarizationCredentials {
    KeychainProfile(String),
    AppleId {
        apple_id: String,
        team_id: String,
        password: String,
    },
    ApiKey {
        key_id: String,
        issuer_id: String,
        key_path: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotarizationSpec {
    pub app_path: PathBuf,
    pub output_dir: PathBuf,
    pub credentials: NotarizationCredentials,
    pub staple: bool,
}

impl NotarizationSpec {
    pub fn new(
        app_path: impl Into<PathBuf>,
        output_dir: impl Into<PathBuf>,
        credentials: NotarizationCredentials,
    ) -> Self {
        Self {
            app_path: app_path.into(),
            output_dir: output_dir.into(),
            credentials,
            staple: true,
        }
    }

    pub fn archive_path(&self) -> Result<PathBuf> {
        let file_name = self
            .app_path
            .file_stem()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                GfmError::Format(format!(
                    "{} does not look like a macOS .app bundle",
                    self.app_path.display()
                ))
            })?;
        Ok(self.output_dir.join(format!("{file_name}-notary.zip")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotarizationStatus {
    Accepted,
    Invalid,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotarizationTicket {
    pub archive_path: PathBuf,
    pub submission_id: String,
    pub status: NotarizationStatus,
    pub stapled: bool,
}

pub fn notarize_app_bundle(spec: &NotarizationSpec) -> Result<NotarizationTicket> {
    validate_spec(spec)?;
    let archive_path = spec.archive_path()?;
    create_archive(&spec.app_path, &archive_path)?;
    let output = submit_archive(&archive_path, &spec.credentials)?;
    let (submission_id, status) = parse_notarytool_submit(&output)?;
    if status != NotarizationStatus::Accepted {
        return Err(GfmError::Format(format!(
            "notarization submission {submission_id} finished with status {status:?}"
        )));
    }
    let stapled = if spec.staple {
        staple_and_validate(&spec.app_path)?;
        true
    } else {
        false
    };
    Ok(NotarizationTicket {
        archive_path,
        submission_id,
        status,
        stapled,
    })
}

fn validate_spec(spec: &NotarizationSpec) -> Result<()> {
    ensure_dir(&spec.app_path)?;
    if spec.app_path.extension().and_then(|ext| ext.to_str()) != Some("app") {
        return Err(GfmError::Format(format!(
            "{} must be a .app bundle",
            spec.app_path.display()
        )));
    }
    create_dir(&spec.output_dir)?;
    match &spec.credentials {
        NotarizationCredentials::KeychainProfile(profile) => {
            ensure_nonempty("keychain profile", profile)?;
        }
        NotarizationCredentials::AppleId {
            apple_id,
            team_id,
            password,
        } => {
            ensure_nonempty("Apple ID", apple_id)?;
            ensure_nonempty("team ID", team_id)?;
            ensure_nonempty("app-specific password", password)?;
        }
        NotarizationCredentials::ApiKey {
            key_id,
            issuer_id,
            key_path,
        } => {
            ensure_nonempty("API key ID", key_id)?;
            ensure_nonempty("API issuer ID", issuer_id)?;
            ensure_file(key_path)?;
        }
    }
    Ok(())
}

fn create_archive(app_path: &Path, archive_path: &Path) -> Result<()> {
    if archive_path.exists() {
        fs::remove_file(archive_path).map_err(|err| GfmError::io(archive_path, err))?;
    }
    let parent = app_path.parent().ok_or_else(|| {
        GfmError::Format(format!(
            "{} must have a parent directory",
            app_path.display()
        ))
    })?;
    let app_name = app_path.file_name().ok_or_else(|| {
        GfmError::Format(format!(
            "{} must have a bundle directory name",
            app_path.display()
        ))
    })?;
    run_command(
        Command::new("xcrun")
            .arg("ditto")
            .arg("-c")
            .arg("-k")
            .arg("--keepParent")
            .arg(app_name)
            .arg(archive_path)
            .current_dir(parent),
        archive_path,
        "archive app for notarization",
    )
}

fn submit_archive(archive_path: &Path, credentials: &NotarizationCredentials) -> Result<String> {
    let mut command = Command::new("xcrun");
    command
        .arg("notarytool")
        .arg("submit")
        .arg(archive_path)
        .arg("--wait")
        .arg("--output-format")
        .arg("json");
    append_credentials(&mut command, credentials);
    output_command(&mut command, archive_path, "submit app for notarization")
}

fn staple_and_validate(app_path: &Path) -> Result<()> {
    run_command(
        Command::new("xcrun")
            .arg("stapler")
            .arg("staple")
            .arg(app_path),
        app_path,
        "staple notarization ticket",
    )?;
    run_command(
        Command::new("xcrun")
            .arg("stapler")
            .arg("validate")
            .arg(app_path),
        app_path,
        "validate stapled notarization ticket",
    )
}

fn append_credentials(command: &mut Command, credentials: &NotarizationCredentials) {
    match credentials {
        NotarizationCredentials::KeychainProfile(profile) => {
            command.arg("--keychain-profile").arg(profile);
        }
        NotarizationCredentials::AppleId {
            apple_id,
            team_id,
            password,
        } => {
            command
                .arg("--apple-id")
                .arg(apple_id)
                .arg("--team-id")
                .arg(team_id)
                .arg("--password")
                .arg(password);
        }
        NotarizationCredentials::ApiKey {
            key_id,
            issuer_id,
            key_path,
        } => {
            command
                .arg("--key")
                .arg(key_path)
                .arg("--key-id")
                .arg(key_id)
                .arg("--issuer")
                .arg(issuer_id);
        }
    }
}

fn parse_notarytool_submit(output: &str) -> Result<(String, NotarizationStatus)> {
    let value: Value = serde_json::from_str(output)
        .map_err(|err| GfmError::Format(format!("invalid notarytool JSON: {err}")))?;
    let submission_id = string_field(&value, &["id", "submissionId"])?;
    let status = string_field(&value, &["status"])?;
    let status = match status.as_str() {
        "Accepted" => NotarizationStatus::Accepted,
        "Invalid" => NotarizationStatus::Invalid,
        "Rejected" => NotarizationStatus::Rejected,
        other => {
            return Err(GfmError::Format(format!(
                "unknown notarytool status `{other}`"
            )))
        }
    };
    Ok((submission_id, status))
}

fn string_field(value: &Value, names: &[&str]) -> Result<String> {
    for name in names {
        if let Some(field) = value.get(*name).and_then(Value::as_str) {
            if !field.trim().is_empty() {
                return Ok(field.to_string());
            }
        }
    }
    Err(GfmError::Format(format!(
        "notarytool response missing string field {}",
        names.join("/")
    )))
}

fn run_command(command: &mut Command, path: &Path, action: &str) -> Result<()> {
    let output = command.output().map_err(|err| GfmError::io(path, err))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_error(action, path, output))
    }
}

fn output_command(command: &mut Command, path: &Path, action: &str) -> Result<String> {
    let output = command.output().map_err(|err| GfmError::io(path, err))?;
    if output.status.success() {
        String::from_utf8(output.stdout)
            .map_err(|err| GfmError::Format(format!("{action} returned non-UTF-8 output: {err}")))
    } else {
        Err(command_error(action, path, output))
    }
}

fn command_error(action: &str, path: &Path, output: std::process::Output) -> GfmError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    GfmError::Format(format!(
        "failed to {action} for {} with status {}; stdout: {}; stderr: {}",
        path.display(),
        output.status,
        stdout.trim(),
        stderr.trim()
    ))
}

fn ensure_nonempty(label: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(GfmError::Format(format!("{label} cannot be empty")))
    } else {
        Ok(())
    }
}

fn create_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|err| GfmError::io(path, err))
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
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parses_accepted_notarytool_output() {
        let (submission_id, status) =
            parse_notarytool_submit(r#"{"id":"abc-123","status":"Accepted"}"#)
                .expect("parse notarytool output");

        assert_eq!(submission_id, "abc-123");
        assert_eq!(status, NotarizationStatus::Accepted);
    }

    #[test]
    fn parses_submission_id_alias() {
        let (submission_id, status) =
            parse_notarytool_submit(r#"{"submissionId":"sub-456","status":"Invalid"}"#)
                .expect("parse notarytool output");

        assert_eq!(submission_id, "sub-456");
        assert_eq!(status, NotarizationStatus::Invalid);
    }

    #[test]
    fn rejects_unknown_notarytool_status() {
        let err = parse_notarytool_submit(r#"{"id":"abc-123","status":"In Progress"}"#)
            .expect_err("unknown status fails");

        assert!(err.to_string().contains("unknown notarytool status"));
    }

    #[test]
    fn validates_keychain_profile_credentials() {
        let root = temp_root("keychain");
        let app = root.join("GFM.app");
        fs::create_dir_all(&app).expect("create app");

        let spec = NotarizationSpec::new(
            &app,
            root.join("dist"),
            NotarizationCredentials::KeychainProfile("release".to_string()),
        );

        validate_spec(&spec).expect("valid spec");
        assert_eq!(
            spec.archive_path().unwrap(),
            root.join("dist/GFM-notary.zip")
        );
    }

    #[test]
    fn rejects_missing_api_key_file() {
        let root = temp_root("api-key");
        let app = root.join("GFM.app");
        fs::create_dir_all(&app).expect("create app");

        let spec = NotarizationSpec::new(
            &app,
            root.join("dist"),
            NotarizationCredentials::ApiKey {
                key_id: "KEYID".to_string(),
                issuer_id: "ISSUER".to_string(),
                key_path: root.join("AuthKey_KEYID.p8"),
            },
        );
        let err = validate_spec(&spec).expect_err("missing key fails");

        assert!(err.to_string().contains("missing"));
    }

    fn temp_root(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        std::env::temp_dir().join(format!("gfm-notarize-{name}-{nonce}"))
    }
}
